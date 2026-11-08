//! Small line icons (24×24, stroke = currentColor) used instead of emoji glyphs.

pub fn icon(name: &str) -> String {
    let body = match name {
        "menu" => "<path d='M4 7h16M4 12h16M4 17h16'/>",
        "undo" => "<path d='M9 14L4 9l5-5'/><path d='M4 9h11a5 5 0 0 1 0 10h-3'/>",
        "redo" => "<path d='M15 14l5-5-5-5'/><path d='M20 9H9a5 5 0 0 0 0 10h3'/>",
        "help" => "<circle cx='12' cy='12' r='9'/><path d='M9.5 9.5a2.5 2.5 0 1 1 3.5 2.3c-.7.3-1 .9-1 1.7'/><path d='M12 17h.01'/>",
        "draw" => "<rect x='6' y='3' width='12' height='16' rx='1.5'/><path d='M12 21v-6M9 18l3 3 3-3'/>",
        "channel" => "<path d='M12 3l7 9-7 9-7-9z'/>",
        "ready" => "<path d='M20 12a8 8 0 1 1-2.3-5.7'/><path d='M20 4v5h-5'/>",
        "shuffle" => "<path d='M4 7h4l8 10h4M4 17h4l2.5-3M13.5 10L16 7h4'/><path d='M18 5l2 2-2 2M18 15l2 2-2 2'/>",
        "dice" => "<rect x='4' y='4' width='16' height='16' rx='3'/><circle cx='9' cy='9' r='1' fill='currentColor'/><circle cx='15' cy='15' r='1' fill='currentColor'/><circle cx='15' cy='9' r='1' fill='currentColor'/><circle cx='9' cy='15' r='1' fill='currentColor'/>",
        "token" => "<circle cx='12' cy='12' r='8'/><path d='M12 8v8M8 12h8'/>",
        "pay" => "<path d='M12 3l7 9-7 9-7-9z'/><path d='M9 12l2 2 4-4'/>",
        "eye" => "<path d='M2 12s3.5-6 10-6 10 6 10 6-3.5 6-10 6S2 12 2 12z'/><circle cx='12' cy='12' r='2.5'/>",
        "duel" => "<path d='M5 3l9 9M3 5l9 9M14 14l3 3M17 14l-3 3M19 3l-9 9M21 5l-9 9M10 14l-3 3M7 14l3 3'/>",
        "solo" => "<circle cx='12' cy='8' r='3.5'/><path d='M5 20c1-4 3.8-6 7-6s6 2 7 6'/>",
        "sealed" => "<path d='M6 3h12v18H6z'/><path d='M6 8h12M10 3v5M14 3v5'/>",
        "deck" => "<rect x='7' y='3' width='11' height='15' rx='1.5'/><path d='M5 6v14h11'/>",
        "rooms" => "<path d='M4 6h16M4 12h16M4 18h10'/>",
        "close" => "<path d='M6 6l12 12M18 6L6 18'/>",
        _ => "",
    };
    format!("<svg class='ic' viewBox='0 0 24 24' fill='none' stroke='currentColor' stroke-width='1.8' stroke-linecap='round' stroke-linejoin='round' aria-hidden='true'>{body}</svg>")
}

/// The Riftbound might symbol (a small sword), inline with text.
pub fn might() -> &'static str {
    "<svg class='ic-m' viewBox='0 0 16 16' aria-label='might'><path d='M12.5 1.5l2 2-7 7-2-2zM5 9.5l1.5 1.5-1 1 1 1-1.5 1.5-1-1-1.5 1.5-1-1 1.5-1.5-1-1L4 9.5z' fill='currentColor'/></svg>"
}

pub fn exhaust() -> &'static str {
    "<svg class='ic-m' viewBox='0 0 16 16' aria-label='exhaust'><path d='M3 8a5 5 0 1 0 1.6-3.7M3 2v3h3' fill='none' stroke='currentColor' stroke-width='1.6' stroke-linecap='round'/></svg>"
}
