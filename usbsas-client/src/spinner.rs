//! Fancy loading spinner (AI generated from the svg file used in the old web
//! client)

use std::f32::consts::TAU;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use iced::mouse;
use iced::widget::canvas::{self, Frame, Geometry, LineCap, Path, Stroke};
use iced::{Color, Element, Length, Point, Rectangle, Renderer, Theme};

/// Duration of one lap (`dur="1s"`).
const PERIOD: Duration = Duration::from_secs(1);

const RX: f32 = 15.0;
const RY: f32 = 35.0;
const TRACK_WIDTH: f32 = 3.0;
const BALL_RADIUS: f32 = 9.0;

/// Distance from the eye to the centre of the spinner, in viewBox units.
/// Smaller = stronger 3D effect. With 120 the balls range from about
/// ×0.79 (far side) to ×1.36 (near side) of `BALL_RADIUS`.
const CAMERA_DISTANCE: f32 = 120.0;

/// Number of segments used to draw the ellipses and to build the arc-length table.
const SAMPLES: usize = 256;

/// Number of pieces each track is cut into for depth sorting.
/// Must divide `SAMPLES`.
const TRACK_SEGMENTS: usize = 128;
const SAMPLES_PER_SEGMENT: usize = SAMPLES / TRACK_SEGMENTS;

/// One entry per ball: rotation of its ellipse in degrees, colour, phase
/// offset.
const BALLS: [(f32, [u8; 3], f32); 3] = [
    (0.0, [0xb7, 0x0e, 0x0b], 0.0),
    (60.0, [0xfc, 0xb4, 0x3a], 1.0 / 6.0),
    (120.0, [0x77, 0xb7, 0x2b], 2.0 / 6.0),
];
const TRACK_COLOR: [u8; 3] = [0x67, 0x72, 0x7c];

/// A self-contained loading spinner.
///
/// Call [`Spinner::tick`] from a `window::frames()` subscription and put
/// [`Spinner::view`] wherever the spinner should appear.
#[derive(Debug, Clone)]
pub struct Spinner {
    start: Instant,
    /// Position in the current lap, in `[0, 1)`.
    phase: f32,
    /// Side of the square widget, in logical pixels.
    size: f32,
}

impl Spinner {
    pub fn new(size: f32) -> Self {
        Self {
            start: Instant::now(),
            phase: 0.0,
            size,
        }
    }

    /// Advance the animation to `now`.
    pub fn tick(&mut self, now: Instant) {
        let elapsed = now.saturating_duration_since(self.start).as_secs_f32();
        self.phase = (elapsed / PERIOD.as_secs_f32()).fract();
    }

    pub fn view<'a, Message: 'a>(&'a self) -> Element<'a, Message> {
        canvas::Canvas::new(self)
            .width(Length::Fixed(self.size))
            .height(Length::Fixed(self.size))
            .into()
    }
}

impl Default for Spinner {
    fn default() -> Self {
        Self::new(200.0)
    }
}

impl<Message> canvas::Program<Message> for Spinner {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());

        // Map viewBox units (origin at the centre) to widget pixels.
        let scale = bounds.width.min(bounds.height) / 100.0;
        let center = frame.center();
        let to_screen = |p: Point| Point::new(center.x + p.x * scale, center.y + p.y * scale);

        // Everything is drawn with the painter's algorithm: each ellipse is cut
        // into short pieces, and pieces and balls are sorted back to front
        // together. That way a ball on the far side of its orbit passes behind
        // the near side of the other two tracks, and in front of their far side.
        let mut items = Vec::with_capacity(BALLS.len() * (TRACK_SEGMENTS + 1));

        for (ellipse, &(rotation, color, offset)) in BALLS.iter().enumerate() {
            for segment in 0..TRACK_SEGMENTS {
                let mid = TAU * (segment as f32 + 0.5) / TRACK_SEGMENTS as f32;
                items.push((depth(mid), Item::Track { ellipse, segment }));
            }

            let theta = paced_angle(self.phase + offset);
            let z = depth(theta);
            // Sort the ball by its front surface rather than its centre, so the
            // bit of its own track that runs "through" it stays hidden.
            items.push((
                z + BALL_RADIUS,
                Item::Ball {
                    rotation,
                    color,
                    theta,
                    z,
                },
            ));
        }

        // Stable sort: on equal depth, tracks (pushed first) stay under the ball.
        items.sort_by(|a, b| a.0.total_cmp(&b.0));

        for (_, item) in items {
            match item {
                Item::Track { ellipse, segment } => {
                    let rotation = BALLS[ellipse].0;
                    let first = segment * SAMPLES_PER_SEGMENT;
                    let mid = TAU * (segment as f32 + 0.5) / TRACK_SEGMENTS as f32;
                    let piece = Path::new(|b| {
                        for i in first..=first + SAMPLES_PER_SEGMENT {
                            let theta = TAU * i as f32 / SAMPLES as f32;
                            let p = to_screen(rotate(ellipse_point(theta), rotation));
                            if i == first {
                                b.move_to(p);
                            } else {
                                b.line_to(p);
                            }
                        }
                    });
                    // Round caps hide the joins between pieces, which can differ
                    // slightly in width because of the perspective.
                    let stroke = Stroke::default()
                        .with_color(rgb(TRACK_COLOR))
                        .with_width(TRACK_WIDTH * perspective(depth(mid)) * scale)
                        .with_line_cap(LineCap::Round);
                    frame.stroke(&piece, stroke);
                }
                Item::Ball {
                    rotation,
                    color,
                    theta,
                    z,
                } => {
                    let position = to_screen(rotate(ellipse_point(theta), rotation));
                    let radius = BALL_RADIUS * perspective(z) * scale;
                    frame.fill(&Path::circle(position, radius), rgb(color));
                }
            }
        }

        vec![frame.into_geometry()]
    }
}

/// Something to draw, once sorted by depth.
#[derive(Debug, Clone, Copy)]
enum Item {
    /// Piece number `segment` of the track at index `ellipse` in `BALLS`.
    Track { ellipse: usize, segment: usize },
    Ball {
        rotation: f32,
        color: [u8; 3],
        theta: f32,
        z: f32,
    },
}

/// Point on the unrotated ellipse, relative to its centre. `theta = 0` is the
/// top; increasing `theta` goes clockwise on screen (y points down), like the
/// SVG arc with sweep-flag 1.
fn ellipse_point(theta: f32) -> Point {
    Point::new(RX * theta.sin(), -RY * theta.cos())
}

/// Depth of the ball, in viewBox units, positive towards the viewer.
///
/// Each ellipse is read as a circle of radius `RY` tilted around its long axis,
/// which is what squashes its width down to `RX`. The out-of-screen component
/// of that circle is `sqrt(RY² - RX²) · sin θ`. The rotation of each ellipse
/// happens in the screen plane, so it doesn't change the depth.
fn depth(theta: f32) -> f32 {
    (RY * RY - RX * RX).sqrt() * theta.sin()
}

/// Size factor for a ball at depth `z`: a simple pinhole projection with the
/// eye `CAMERA_DISTANCE` units in front of the centre.
fn perspective(z: f32) -> f32 {
    CAMERA_DISTANCE / (CAMERA_DISTANCE - z)
}

/// Rotate around the centre, clockwise on screen for positive degrees
fn rotate(p: Point, degrees: f32) -> Point {
    let (sin, cos) = degrees.to_radians().sin_cos();
    Point::new(p.x * cos - p.y * sin, p.x * sin + p.y * cos)
}

/// Cumulative arc length of the ellipse at each of the `SAMPLES + 1` sample angles.
static ARC_LENGTHS: LazyLock<Vec<f32>> = LazyLock::new(|| {
    let mut lengths = Vec::with_capacity(SAMPLES + 1);
    let mut total = 0.0;
    let mut prev = ellipse_point(0.0);
    lengths.push(0.0);
    for i in 1..=SAMPLES {
        let p = ellipse_point(TAU * i as f32 / SAMPLES as f32);
        total += prev.distance(p);
        lengths.push(total);
        prev = p;
    }
    lengths
});

/// Ellipse angle reached after travelling `fraction` of the perimeter at
/// constant speed (what `animateMotion` does by default).
fn paced_angle(fraction: f32) -> f32 {
    let lengths = &*ARC_LENGTHS;
    let target = fraction.rem_euclid(1.0) * lengths[SAMPLES];
    let i = lengths.partition_point(|&l| l < target).clamp(1, SAMPLES);
    let (a, b) = (lengths[i - 1], lengths[i]);
    let t = if b > a { (target - a) / (b - a) } else { 0.0 };
    TAU * (i as f32 - 1.0 + t) / SAMPLES as f32
}

fn rgb([r, g, b]: [u8; 3]) -> Color {
    Color::from_rgb8(r, g, b)
}
