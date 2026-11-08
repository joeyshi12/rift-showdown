//! Minimal 3D math: column-major 4x4 matrices and vectors.

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct V3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

pub const fn v3(x: f32, y: f32, z: f32) -> V3 {
    V3 { x, y, z }
}

impl V3 {
    pub fn add(self, o: V3) -> V3 {
        v3(self.x + o.x, self.y + o.y, self.z + o.z)
    }
    pub fn sub(self, o: V3) -> V3 {
        v3(self.x - o.x, self.y - o.y, self.z - o.z)
    }
    pub fn mul(self, s: f32) -> V3 {
        v3(self.x * s, self.y * s, self.z * s)
    }
    pub fn dot(self, o: V3) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }
    pub fn cross(self, o: V3) -> V3 {
        v3(self.y * o.z - self.z * o.y, self.z * o.x - self.x * o.z, self.x * o.y - self.y * o.x)
    }
    pub fn len(self) -> f32 {
        self.dot(self).sqrt()
    }
    pub fn norm(self) -> V3 {
        let l = self.len().max(1e-6);
        self.mul(1.0 / l)
    }
    pub fn lerp(self, o: V3, t: f32) -> V3 {
        self.add(o.sub(self).mul(t))
    }
}

pub type M4 = [f32; 16];

pub fn ident() -> M4 {
    [1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.]
}

pub fn mul(a: &M4, b: &M4) -> M4 {
    let mut r = [0f32; 16];
    for c in 0..4 {
        for row in 0..4 {
            let mut s = 0.0;
            for k in 0..4 {
                s += a[k * 4 + row] * b[c * 4 + k];
            }
            r[c * 4 + row] = s;
        }
    }
    r
}

pub fn perspective(fovy: f32, aspect: f32, near: f32, far: f32) -> M4 {
    let f = 1.0 / (fovy / 2.0).tan();
    let nf = 1.0 / (near - far);
    [f / aspect, 0., 0., 0., 0., f, 0., 0., 0., 0., (far + near) * nf, -1., 0., 0., 2. * far * near * nf, 0.]
}

pub fn look_at(eye: V3, target: V3, up: V3) -> M4 {
    let f = target.sub(eye).norm();
    let s = f.cross(up).norm();
    let u = s.cross(f);
    [s.x, u.x, -f.x, 0., s.y, u.y, -f.y, 0., s.z, u.z, -f.z, 0., -s.dot(eye), -u.dot(eye), f.dot(eye), 1.]
}

pub fn translate(t: V3) -> M4 {
    let mut m = ident();
    m[12] = t.x;
    m[13] = t.y;
    m[14] = t.z;
    m
}

pub fn scale(x: f32, y: f32, z: f32) -> M4 {
    let mut m = ident();
    m[0] = x;
    m[5] = y;
    m[10] = z;
    m
}

pub fn rot_y(a: f32) -> M4 {
    let (s, c) = a.sin_cos();
    [c, 0., -s, 0., 0., 1., 0., 0., s, 0., c, 0., 0., 0., 0., 1.]
}

pub fn rot_x(a: f32) -> M4 {
    let (s, c) = a.sin_cos();
    [1., 0., 0., 0., 0., c, s, 0., 0., -s, c, 0., 0., 0., 0., 1.]
}

pub fn rot_z(a: f32) -> M4 {
    let (s, c) = a.sin_cos();
    [c, s, 0., 0., -s, c, 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.]
}

pub fn transform(m: &M4, p: V3) -> (f32, f32, f32, f32) {
    (
        m[0] * p.x + m[4] * p.y + m[8] * p.z + m[12],
        m[1] * p.x + m[5] * p.y + m[9] * p.z + m[13],
        m[2] * p.x + m[6] * p.y + m[10] * p.z + m[14],
        m[3] * p.x + m[7] * p.y + m[11] * p.z + m[15],
    )
}

/// Shortest-path angle interpolation.
pub fn lerp_angle(a: f32, b: f32, t: f32) -> f32 {
    let mut d = (b - a) % std::f32::consts::TAU;
    if d > std::f32::consts::PI {
        d -= std::f32::consts::TAU;
    }
    if d < -std::f32::consts::PI {
        d += std::f32::consts::TAU;
    }
    a + d * t
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn projection_roundtrip() {
        let vp = mul(&perspective(0.8, 1.5, 0.1, 100.0), &look_at(v3(0., 10., 10.), v3(0., 0., 0.), v3(0., 1., 0.)));
        let (x, y, _, w) = transform(&vp, v3(0., 0., 0.));
        assert!((x / w).abs() < 1e-4 && (y / w).abs() < 1e-4);
        let (_, y2, _, w2) = transform(&vp, v3(0., 0., 3.));
        assert!(y2 / w2 < 0.0, "nearer points appear lower on screen");
    }
}
