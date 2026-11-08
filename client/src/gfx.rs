//! WebGL2 renderer: textured rounded card quads, glows, shadows and the table.

use crate::dom;
use crate::math::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement, HtmlImageElement, WebGl2RenderingContext as GL, WebGlProgram, WebGlTexture, WebGlUniformLocation};

/// Portrait camera: how much closer it comes, and how far behind the table it sits
/// (10.6 on landscape screens; lower is more top-down).
const PORTRAIT_ZOOM: f32 = 0.12;
const PORTRAIT_BACK: f32 = 4.0;

const VS: &str = r#"#version 300 es
layout(location=0) in vec3 a_pos;
layout(location=1) in vec2 a_uv;
uniform mat4 u_mvp;
uniform mat4 u_model;
out vec2 v_uv;
out vec3 v_world;
void main(){
  v_uv = a_uv;
  vec4 w = u_model * vec4(a_pos,1.0);
  v_world = w.xyz;
  gl_Position = u_mvp * vec4(a_pos,1.0);
}"#;

const FS: &str = r#"#version 300 es
precision highp float;
in vec2 v_uv;
in vec3 v_world;
uniform sampler2D u_tex;
uniform vec4 u_tint;
uniform int u_mode;
uniform vec2 u_size;
uniform float u_time;
uniform vec4 u_uvrect;
out vec4 o;
float rbox(vec2 p, vec2 b, float r){ vec2 q=abs(p)-b+r; return length(max(q,0.0))+min(max(q.x,q.y),0.0)-r; }
void main(){
  vec2 p = (v_uv-0.5)*u_size;
  float r = 0.055*min(u_size.x,u_size.y);
  if(u_mode==0){
    float d = rbox(p, u_size*0.5, r);
    float a = 1.0 - smoothstep(-0.012, 0.0, d);
    vec2 uv = u_uvrect.xy + v_uv*u_uvrect.zw;
    vec4 c = texture(u_tex, uv);
    float edge = smoothstep(-0.035, -0.012, d);
    vec3 col = mix(c.rgb, vec3(0.05,0.06,0.09), edge*0.85);
    // subtle sheen sweeping across cards
    float sheen = 0.0;
    o = vec4(col*u_tint.rgb + sheen, a*c.a*u_tint.a);
  } else if(u_mode==1){
    float d = rbox(p, u_size*0.5-0.12, r);
    float a = exp(-max(d,0.0)*14.0) * (1.0 - smoothstep(0.0,0.12,d));
    float pulse = 0.75 + 0.25*sin(u_time*5.0);
    o = vec4(u_tint.rgb, a*u_tint.a*pulse);
  } else if(u_mode==2){
    o = texture(u_tex, v_uv) * u_tint;
  } else if(u_mode==3){
    float d = rbox(p, u_size*0.5-0.15, r);
    float a = 1.0 - smoothstep(-0.10, 0.16, d);
    o = vec4(0.0,0.0,0.0,a*0.55*u_tint.a);
  } else {
    // flat rounded panel (zone highlight)
    float d = rbox(p, u_size*0.5, r*1.4);
    float a = (1.0 - smoothstep(-0.01,0.0,d));
    float ring = 1.0 - smoothstep(0.0,0.05, abs(d+0.03));
    o = vec4(u_tint.rgb, (a*0.22 + ring*0.9)*u_tint.a);
  }
}"#;

pub struct Tex {
    pub tex: WebGlTexture,
    pub ready: bool,
}

pub struct Renderer {
    pub gl: GL,
    pub canvas: HtmlCanvasElement,
    prog: WebGlProgram,
    u_mvp: Option<WebGlUniformLocation>,
    u_model: Option<WebGlUniformLocation>,
    u_tint: Option<WebGlUniformLocation>,
    u_mode: Option<WebGlUniformLocation>,
    u_size: Option<WebGlUniformLocation>,
    u_time: Option<WebGlUniformLocation>,
    u_uvrect: Option<WebGlUniformLocation>,
    pub textures: Rc<RefCell<HashMap<String, Tex>>>,
    pub back: WebGlTexture,
    pub rune_back: WebGlTexture,
    pub white: WebGlTexture,
    pub table: Option<WebGlTexture>,
    pub table_key: String,
    pub view_proj: M4,
    pub eye: V3,
    pub target: V3,
    pub fovy: f32,
    pub aspect: f32,
    pub time: f32,
    aniso: Option<(u32, f32)>,
    pending: Rc<RefCell<usize>>,
}

fn compile(gl: &GL, ty: u32, src: &str) -> Result<web_sys::WebGlShader, String> {
    let s = gl.create_shader(ty).ok_or("shader")?;
    gl.shader_source(&s, src);
    gl.compile_shader(&s);
    if gl.get_shader_parameter(&s, GL::COMPILE_STATUS).as_bool().unwrap_or(false) {
        Ok(s)
    } else {
        Err(gl.get_shader_info_log(&s).unwrap_or_default())
    }
}

pub fn canvas_2d(w: u32, h: u32) -> (HtmlCanvasElement, CanvasRenderingContext2d) {
    let c: HtmlCanvasElement = dom::document().create_element("canvas").unwrap().dyn_into().unwrap();
    c.set_width(w);
    c.set_height(h);
    let ctx: CanvasRenderingContext2d = c.get_context("2d").unwrap().unwrap().dyn_into().unwrap();
    (c, ctx)
}

impl Renderer {
    pub fn new(canvas: HtmlCanvasElement) -> Result<Renderer, String> {
        let opts = js_sys::Object::new();
        let _ = js_sys::Reflect::set(&opts, &"antialias".into(), &true.into());
        let _ = js_sys::Reflect::set(&opts, &"alpha".into(), &false.into());
        let _ = js_sys::Reflect::set(&opts, &"preserveDrawingBuffer".into(), &true.into());
        let gl: GL = canvas.get_context_with_context_options("webgl2", &opts).map_err(|_| "webgl2 unavailable")?.ok_or("WebGL2 is not supported by this browser")?.dyn_into().map_err(|_| "webgl2 cast")?;
        let vs = compile(&gl, GL::VERTEX_SHADER, VS)?;
        let fs = compile(&gl, GL::FRAGMENT_SHADER, FS)?;
        let prog = gl.create_program().ok_or("program")?;
        gl.attach_shader(&prog, &vs);
        gl.attach_shader(&prog, &fs);
        gl.link_program(&prog);
        if !gl.get_program_parameter(&prog, GL::LINK_STATUS).as_bool().unwrap_or(false) {
            return Err(gl.get_program_info_log(&prog).unwrap_or_default());
        }
        gl.use_program(Some(&prog));
        // Unit quad on the XZ plane, facing +Y. UV v=0 at far (-z) edge (card top).
        let verts: [f32; 20] = [-0.5, 0.0, -0.5, 0.0, 0.0, 0.5, 0.0, -0.5, 1.0, 0.0, -0.5, 0.0, 0.5, 0.0, 1.0, 0.5, 0.0, 0.5, 1.0, 1.0];
        let vao = gl.create_vertex_array().ok_or("vao")?;
        gl.bind_vertex_array(Some(&vao));
        let buf = gl.create_buffer().ok_or("buf")?;
        gl.bind_buffer(GL::ARRAY_BUFFER, Some(&buf));
        let arr = js_sys::Float32Array::from(&verts[..]);
        gl.buffer_data_with_array_buffer_view(GL::ARRAY_BUFFER, &arr, GL::STATIC_DRAW);
        gl.enable_vertex_attrib_array(0);
        gl.vertex_attrib_pointer_with_i32(0, 3, GL::FLOAT, false, 20, 0);
        gl.enable_vertex_attrib_array(1);
        gl.vertex_attrib_pointer_with_i32(1, 2, GL::FLOAT, false, 20, 12);
        gl.enable(GL::BLEND);
        gl.blend_func(GL::SRC_ALPHA, GL::ONE_MINUS_SRC_ALPHA);
        gl.pixel_storei(GL::UNPACK_FLIP_Y_WEBGL, 0);
        let aniso = gl.get_extension("EXT_texture_filter_anisotropic").ok().flatten().map(|_| {
            let max = gl.get_parameter(0x84FF).ok().and_then(|v| v.as_f64()).unwrap_or(1.0) as f32;
            (0x84FEu32, max.min(8.0))
        });
        let u = |n: &str| gl.get_uniform_location(&prog, n);
        let white = {
            let (c, ctx) = canvas_2d(4, 4);
            ctx.set_fill_style_str("#fff");
            ctx.fill_rect(0.0, 0.0, 4.0, 4.0);
            upload_canvas(&gl, &c, None)
        };
        let back = upload_canvas(&gl, &card_back(false), aniso);
        let rune_back = upload_canvas(&gl, &card_back(true), aniso);
        Ok(Renderer {
            u_mvp: u("u_mvp"),
            u_model: u("u_model"),
            u_tint: u("u_tint"),
            u_mode: u("u_mode"),
            u_size: u("u_size"),
            u_time: u("u_time"),
            u_uvrect: u("u_uvrect"),
            gl,
            canvas,
            prog,
            textures: Rc::new(RefCell::new(HashMap::new())),
            back,
            rune_back,
            white,
            table: None,
            table_key: String::new(),
            view_proj: ident(),
            eye: v3(0., 14., 12.),
            target: v3(0., 0., 1.),
            fovy: 0.72,
            aspect: 1.6,
            time: 0.0,
            aniso,
            pending: Rc::new(RefCell::new(0)),
        })
    }

    pub fn pending_images(&self) -> usize {
        *self.pending.borrow()
    }

    /// Resize the drawing buffer to the canvas' CSS size and set up the camera.
    /// `zoom` widens the view for the four-player table (1.0 = the 1v1 table).
    pub fn begin(&mut self, flip: bool, zoom: f32) {
        let dpr = dom::window().device_pixel_ratio().min(2.0);
        let w = (self.canvas.client_width() as f64 * dpr) as u32;
        let h = (self.canvas.client_height() as f64 * dpr) as u32;
        if w > 0 && h > 0 && (self.canvas.width() != w || self.canvas.height() != h) {
            self.canvas.set_width(w);
            self.canvas.set_height(h);
        }
        let (w, h) = (self.canvas.width().max(1), self.canvas.height().max(1));
        self.gl.viewport(0, 0, w as i32, h as i32);
        self.aspect = w as f32 / h as f32;
        // Pull the camera back on narrow screens so the whole table fits. On portrait
        // screens the width is the limit, so look down more steeply: the table then uses
        // the spare height instead of shrinking into a thin strip.
        let portrait = ((1.3 - self.aspect) / 0.8).clamp(0.0, 1.0);
        let fit = (1.75 * zoom / self.aspect).max(1.0) * (1.0 - PORTRAIT_ZOOM * portrait);
        let dist = 1.0 * fit;
        let sgn = if flip { -1.0 } else { 1.0 };
        let back = 10.6 + (PORTRAIT_BACK - 10.6) * portrait;
        self.eye = v3(0.0, 15.2 * dist, back * dist * sgn);
        self.target = v3(0.0, 0.0, 1.55 * (1.0 - portrait) * sgn);
        let up = v3(0.0, 1.0, 0.0);
        self.view_proj = mul(&perspective(self.fovy, self.aspect, 0.5, 120.0), &look_at(self.eye, self.target, up));
        self.gl.clear_color(0.004, 0.016, 0.043, 1.0);
        self.gl.clear(GL::COLOR_BUFFER_BIT | GL::DEPTH_BUFFER_BIT);
        self.gl.use_program(Some(&self.prog));
        self.gl.uniform1f(self.u_time.as_ref(), self.time);
    }

    pub fn draw(&self, model: &M4, tex: &WebGlTexture, mode: i32, tint: [f32; 4], size: (f32, f32)) {
        self.draw_uv(model, tex, mode, tint, size, [0.0, 0.0, 1.0, 1.0]);
    }

    pub fn draw_uv(&self, model: &M4, tex: &WebGlTexture, mode: i32, tint: [f32; 4], size: (f32, f32), uv: [f32; 4]) {
        let gl = &self.gl;
        let mvp = mul(&self.view_proj, model);
        gl.uniform_matrix4fv_with_f32_array(self.u_mvp.as_ref(), false, &mvp);
        gl.uniform_matrix4fv_with_f32_array(self.u_model.as_ref(), false, model);
        gl.uniform4f(self.u_tint.as_ref(), tint[0], tint[1], tint[2], tint[3]);
        gl.uniform1i(self.u_mode.as_ref(), mode);
        gl.uniform2f(self.u_size.as_ref(), size.0, size.1);
        gl.uniform4f(self.u_uvrect.as_ref(), uv[0], uv[1], uv[2], uv[3]);
        gl.active_texture(GL::TEXTURE0);
        gl.bind_texture(GL::TEXTURE_2D, Some(tex));
        gl.draw_arrays(GL::TRIANGLE_STRIP, 0, 4);
    }

    /// Project a world point to CSS pixels.
    pub fn to_screen(&self, p: V3) -> Option<(f64, f64)> {
        let (x, y, _, w) = transform(&self.view_proj, p);
        if w <= 0.01 {
            return None;
        }
        let cw = self.canvas.client_width() as f64;
        let ch = self.canvas.client_height() as f64;
        Some((((x / w) as f64 * 0.5 + 0.5) * cw, (1.0 - ((y / w) as f64 * 0.5 + 0.5)) * ch))
    }

    /// Intersect the ray through a CSS pixel with the horizontal plane y = h.
    pub fn pick_plane(&self, sx: f64, sy: f64, h: f32) -> Option<(f32, f32)> {
        let cw = self.canvas.client_width() as f32;
        let ch = self.canvas.client_height() as f32;
        let nx = (sx as f32 / cw) * 2.0 - 1.0;
        let ny = 1.0 - (sy as f32 / ch) * 2.0;
        let f = self.target.sub(self.eye).norm();
        let r = f.cross(v3(0., 1., 0.)).norm();
        let u = r.cross(f);
        let t = (self.fovy / 2.0).tan();
        let dir = f.add(r.mul(nx * t * self.aspect)).add(u.mul(ny * t)).norm();
        if dir.y.abs() < 1e-5 {
            return None;
        }
        let k = (h - self.eye.y) / dir.y;
        if k < 0.0 {
            return None;
        }
        let p = self.eye.add(dir.mul(k));
        Some((p.x, p.z))
    }

    /// Texture for a card image URL; starts loading on first request.
    pub fn texture(&self, key: &str, url: &str, fallback_label: &str, landscape: bool) -> Option<WebGlTexture> {
        if let Some(t) = self.textures.borrow().get(key) {
            return if t.ready { Some(t.tex.clone()) } else { None };
        }
        let gl = self.gl.clone();
        let tex = gl.create_texture()?;
        self.textures.borrow_mut().insert(key.to_string(), Tex { tex: tex.clone(), ready: false });
        let img = HtmlImageElement::new().ok()?;
        let texs = self.textures.clone();
        let key2 = key.to_string();
        let img2 = img.clone();
        let aniso = self.aniso;
        let pending = self.pending.clone();
        *pending.borrow_mut() += 1;
        let p2 = pending.clone();
        let onload = Closure::once_into_js(move || {
            gl.bind_texture(GL::TEXTURE_2D, Some(&tex));
            let _ = gl.tex_image_2d_with_u32_and_u32_and_html_image_element(GL::TEXTURE_2D, 0, GL::RGBA as i32, GL::RGBA, GL::UNSIGNED_BYTE, &img2);
            set_params(&gl, aniso);
            if let Some(t) = texs.borrow_mut().get_mut(&key2) {
                t.ready = true;
            }
            *p2.borrow_mut() -= 1;
        });
        let gl3 = self.gl.clone();
        let texs3 = self.textures.clone();
        let key3 = key.to_string();
        let label = fallback_label.to_string();
        let p3 = pending.clone();
        let onerr = Closure::once_into_js(move || {
            let c = placeholder(&label, landscape);
            if let Some(t) = texs3.borrow_mut().get_mut(&key3) {
                gl3.bind_texture(GL::TEXTURE_2D, Some(&t.tex));
                let _ = gl3.tex_image_2d_with_u32_and_u32_and_html_canvas_element(GL::TEXTURE_2D, 0, GL::RGBA as i32, GL::RGBA, GL::UNSIGNED_BYTE, &c);
                set_params(&gl3, aniso);
                t.ready = true;
            }
            *p3.borrow_mut() -= 1;
        });
        img.set_onload(Some(onload.unchecked_ref()));
        img.set_onerror(Some(onerr.unchecked_ref()));
        if url.is_empty() {
            img.set_src("data:,");
        } else {
            img.set_src(url);
        }
        None
    }

    pub fn set_table(&mut self, key: &str, canvas: &HtmlCanvasElement) {
        if self.table_key == key {
            return;
        }
        self.table = Some(upload_canvas(&self.gl, canvas, self.aniso));
        self.table_key = key.to_string();
    }
}

fn set_params(gl: &GL, aniso: Option<(u32, f32)>) {
    gl.generate_mipmap(GL::TEXTURE_2D);
    gl.tex_parameteri(GL::TEXTURE_2D, GL::TEXTURE_MIN_FILTER, GL::LINEAR_MIPMAP_LINEAR as i32);
    gl.tex_parameteri(GL::TEXTURE_2D, GL::TEXTURE_MAG_FILTER, GL::LINEAR as i32);
    gl.tex_parameteri(GL::TEXTURE_2D, GL::TEXTURE_WRAP_S, GL::CLAMP_TO_EDGE as i32);
    gl.tex_parameteri(GL::TEXTURE_2D, GL::TEXTURE_WRAP_T, GL::CLAMP_TO_EDGE as i32);
    if let Some((p, v)) = aniso {
        gl.tex_parameterf(GL::TEXTURE_2D, p, v);
    }
}

pub fn upload_canvas(gl: &GL, c: &HtmlCanvasElement, aniso: Option<(u32, f32)>) -> WebGlTexture {
    let t = gl.create_texture().unwrap();
    gl.bind_texture(GL::TEXTURE_2D, Some(&t));
    let _ = gl.tex_image_2d_with_u32_and_u32_and_html_canvas_element(GL::TEXTURE_2D, 0, GL::RGBA as i32, GL::RGBA, GL::UNSIGNED_BYTE, c);
    set_params(gl, aniso);
    t
}

/// Procedural card back: deep blue with gold rune circle.
pub fn card_back(rune: bool) -> HtmlCanvasElement {
    let (w, h) = (360.0, 504.0);
    let (c, x) = canvas_2d(w as u32, h as u32);
    let g = x.create_linear_gradient(0.0, 0.0, w, h);
    if rune {
        let _ = g.add_color_stop(0.0, "#2a1546");
        let _ = g.add_color_stop(1.0, "#0c0718");
    } else {
        let _ = g.add_color_stop(0.0, "#0e2a52");
        let _ = g.add_color_stop(1.0, "#050c1d");
    }
    x.set_fill_style_canvas_gradient(&g);
    x.fill_rect(0.0, 0.0, w, h);
    x.set_stroke_style_str("rgba(222,186,104,0.9)");
    x.set_line_width(10.0);
    x.stroke_rect(16.0, 16.0, w - 32.0, h - 32.0);
    x.set_line_width(2.0);
    x.stroke_rect(32.0, 32.0, w - 64.0, h - 64.0);
    let (cx, cy) = (w / 2.0, h / 2.0);
    for (r, lw) in [(118.0, 6.0), (98.0, 2.0), (60.0, 3.0)] {
        x.set_line_width(lw);
        x.begin_path();
        let _ = x.arc(cx, cy, r, 0.0, std::f64::consts::TAU);
        x.stroke();
    }
    // star / rune spokes
    x.begin_path();
    for i in 0..8 {
        let a = i as f64 * std::f64::consts::TAU / 8.0;
        let (r1, r2) = (60.0, 118.0);
        x.move_to(cx + a.cos() * r1, cy + a.sin() * r1);
        x.line_to(cx + a.cos() * r2, cy + a.sin() * r2);
    }
    x.stroke();
    x.set_fill_style_str("rgba(222,186,104,0.95)");
    x.set_font("bold 44px Georgia, serif");
    x.set_text_align("center");
    x.set_text_baseline("middle");
    let _ = x.fill_text(if rune { "RUNE" } else { "RIFT" }, cx, cy);
    x.set_font("bold 20px Georgia, serif");
    let _ = x.fill_text("ONLINE", cx, cy + 165.0);
    c
}

/// Fallback face when an image cannot be fetched: name on a dark card.
pub fn placeholder(label: &str, landscape: bool) -> HtmlCanvasElement {
    let (w, h) = if landscape { (504.0, 360.0) } else { (360.0, 504.0) };
    let (c, x) = canvas_2d(w as u32, h as u32);
    let g = x.create_linear_gradient(0.0, 0.0, 0.0, h);
    let _ = g.add_color_stop(0.0, "#24324a");
    let _ = g.add_color_stop(1.0, "#0b1220");
    x.set_fill_style_canvas_gradient(&g);
    x.fill_rect(0.0, 0.0, w, h);
    x.set_stroke_style_str("#8fb8ff");
    x.set_line_width(6.0);
    x.stroke_rect(10.0, 10.0, w - 20.0, h - 20.0);
    x.set_fill_style_str("#e8f0ff");
    x.set_font("bold 30px sans-serif");
    x.set_text_align("center");
    let mut line = String::new();
    let mut y = h * 0.4;
    for word in label.split_whitespace() {
        if line.len() + word.len() > 16 {
            let _ = x.fill_text(&line, w / 2.0, y);
            y += 38.0;
            line.clear();
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    let _ = x.fill_text(&line, w / 2.0, y);
    c
}
