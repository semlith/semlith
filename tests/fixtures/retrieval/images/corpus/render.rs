//! Drawing primitives, so a code file competes with the pictures beside it.
//!
//! Nothing imports this. It is corpus: the keyword list will rank it for
//! `circle`, `square`, `triangle` and `gradient`, and the image list has to
//! beat it for a query that is asking about a picture rather than about code.

/// A colour, as the eight images in this directory use it.
pub struct Rgb(pub u8, pub u8, pub u8);

pub const RED: Rgb = Rgb(220, 38, 38);
pub const BLUE: Rgb = Rgb(37, 99, 235);
pub const GREEN: Rgb = Rgb(22, 163, 74);
pub const PURPLE: Rgb = Rgb(147, 51, 234);
pub const ORANGE: Rgb = Rgb(234, 88, 12);

/// Whether a point falls inside a circle of radius `r` centred at `(cx, cy)`.
pub fn in_circle(x: i32, y: i32, cx: i32, cy: i32, r: i32) -> bool {
    (x - cx).pow(2) + (y - cy).pow(2) <= r * r
}

/// A filled square, given its top-left corner and its side.
pub fn in_square(x: i32, y: i32, left: i32, top: i32, side: i32) -> bool {
    x >= left && x < left + side && y >= top && y < top + side
}

/// An upright triangle: the half-width grows with the distance from the apex.
pub fn in_triangle(x: i32, y: i32, apex_x: i32, apex_y: i32, height: i32, base: i32) -> bool {
    if y < apex_y || y >= apex_y + height {
        return false;
    }
    let half = (y - apex_y) * (base / 2) / height;
    (x - apex_x).abs() <= half
}

/// A ring, which is a circle with a smaller circle taken out of its middle.
pub fn in_ring(x: i32, y: i32, cx: i32, cy: i32, outer: i32, inner: i32) -> bool {
    in_circle(x, y, cx, cy, outer) && !in_circle(x, y, cx, cy, inner)
}

/// One step of a linear gradient from `from` to `to` across `span` pixels.
pub fn gradient_at(offset: i32, span: i32, from: u8, to: u8) -> u8 {
    let range = to as i32 - from as i32;
    (from as i32 + offset * range / span.max(1)) as u8
}

/// Alternating cells, which is what a checkerboard is.
pub fn checker(x: i32, y: i32, cell: i32) -> bool {
    ((x / cell) + (y / cell)) % 2 == 0
}

/// Horizontal stripes: a band on, a band off, down the frame.
pub fn stripe(y: i32, band: i32) -> bool {
    (y / band) % 2 == 0
}

/// A diagonal line of the given thickness, corner to corner.
pub fn on_diagonal(x: i32, y: i32, thickness: i32) -> bool {
    (x - y).abs() <= thickness
}
