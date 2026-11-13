//! The window look of Yggdrasil's Hyprland session (packages/session/hyprland/yggdrasil.lua), drawn by this compositor:
//!
//! - a 1-pixel border in the accent colour (Settings > Appearance; Yggdrasil's purple when none is chosen) at 80 % for the
//!   window in front, a faint one for the others (white at 8 % in the dark style, black at 9 % in the light one);
//! - corners rounded at 10 pixels (the window's picture clipped, the border following the curve);
//! - a shadow 18 pixels wide, black at 27 % against the window fading out with the third power (`render_power = 3`);
//! - gaps: 8 pixels around a maximized window and the tiled ones, 4 on each side between tiled ones (`gaps_out`, `gaps_in`);
//! - animations (Hyprland's defaults, which the session keeps): a window opens growing from its middle and fading in, closes
//!   shrinking and fading out, and slides to a new place and size (maximize, snapping, tiling, keep-clear), each in 800 ms
//!   along Hyprland's "default" curve; none with "reduce motion" (panel.conf, Settings > Accessibility).
//!
//! A full screen window gets none of it, as on Hyprland. Everything is drawn in software too (the headless backend's pixman):
//! the border's corners and the shadow are small pictures made once per scale and colour and stretched along the edges; the
//! rounded corners are a clip of what the program drew (whole pixels by the damage regions, the pixels on the curve drawn once
//! more at their share of coverage). Nothing is drawn while nothing changes; an animation draws at most 60 frames a second
//! while it runs.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement};
use smithay::backend::renderer::element::surface::{WaylandSurfaceRenderElement, WaylandSurfaceTexture};
use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement};
use smithay::backend::renderer::utils::{CommitCounter, DamageSet, OpaqueRegions};
use smithay::backend::renderer::{Color32F, Frame, ImportAll, ImportMem, Renderer, Texture};
use smithay::utils::{Buffer as BufferCoords, Logical, Physical, Point, Rectangle, Scale, Size, Transform};

/// How long each animation runs (Hyprland's `global` speed 8: 8 x 100 ms).
pub const DURATION: Duration = Duration::from_millis(800);

/// The session's look, from its config and the appearance settings.
#[derive(Clone, Debug, PartialEq)]
pub struct Look {
    pub gaps_in: i32,
    pub gaps_out: i32,
    pub border: i32,
    pub rounding: i32,
    pub shadow_range: i32,
    pub shadow_power: i32,
    /// RGBA, not premultiplied
    pub shadow: [u8; 4],
    pub active: [u8; 4],
    pub inactive: [u8; 4],
    /// Animations on (no "reduce motion").
    pub animate: bool,
}

impl Look {
    /// The look for an accent, the dark or light style and "reduce motion".
    pub fn new(accent: Option<[u8; 3]>, dark: bool, reduce_motion: bool) -> Look {
        // Yggdrasil's purple when no accent is chosen, as the session's config
        let [r, g, b] = accent.unwrap_or([0x7c, 0x54, 0xd6]);
        Look {
            gaps_in: 4,
            gaps_out: 8,
            border: 1,
            rounding: 10,
            shadow_range: 18,
            shadow_power: 3,
            shadow: [0, 0, 0, 0x44],
            active: [r, g, b, 0xcc],
            inactive: if dark { [255, 255, 255, 0x14] } else { [0, 0, 0, 0x18] },
            animate: !reduce_motion,
        }
    }

    /// From appearance.conf (accent, style) and panel.conf (reduce motion).
    pub fn load() -> Look {
        let a = yggdrasil_conf::appearance::load();
        Look::new(a.accent, a.dark, yggdrasil_conf::PanelConf::load().reduce_motion)
    }

    /// The files it is read from, to read it again when one changes.
    pub fn files() -> [Option<std::path::PathBuf>; 2] {
        [Some(yggdrasil_conf::appearance::path()), yggdrasil_conf::PanelConf::path()]
    }
}

/// Hyprland's "default" curve: a cubic Bézier through (0, 0.75) and (0.15, 1). `t` from 0 to 1, the share of the way.
pub fn ease(t: f64) -> f64 {
    let (x1, y1, x2, y2) = (0.0, 0.75, 0.15, 1.0);
    if t <= 0.0 {
        return 0.0;
    }
    if t >= 1.0 {
        return 1.0;
    }
    let bez = |a: f64, b: f64, s: f64| 3.0 * a * s * (1.0 - s) * (1.0 - s) + 3.0 * b * s * s * (1.0 - s) + s * s * s;
    // the curve's parameter for this x (x grows with the parameter): halving the interval
    let (mut lo, mut hi) = (0.0, 1.0);
    for _ in 0..40 {
        let mid = (lo + hi) / 2.0;
        if bez(x1, x2, mid) < t {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    bez(y1, y2, (lo + hi) / 2.0)
}

/// The clock animations run on. Tests stop it and move it on by hand (the control socket's `clock`), so a frame in the middle
/// of an animation can be looked at.
pub struct Clock {
    start: Instant,
    stopped: Option<Duration>,
}

impl Default for Clock {
    fn default() -> Self {
        Clock { start: Instant::now(), stopped: None }
    }
}

impl Clock {
    pub fn now(&self) -> Duration {
        self.stopped.unwrap_or_else(|| self.start.elapsed())
    }
    pub fn running(&self) -> bool {
        self.stopped.is_none()
    }
    pub fn stop(&mut self) {
        self.stopped = Some(self.now());
    }
    pub fn run(&mut self) {
        if let Some(t) = self.stopped.take() {
            self.start = Instant::now() - t;
        }
    }
    /// Move a stopped clock on.
    pub fn advance(&mut self, by: Duration) {
        if let Some(t) = &mut self.stopped {
            *t += by;
        }
    }
}

/// A rectangle in logical coordinates with fractions (where a window is drawn during an animation).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RectF {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl RectF {
    pub fn of(r: Rectangle<i32, Logical>) -> RectF {
        RectF { x: r.loc.x as f64, y: r.loc.y as f64, w: r.size.w as f64, h: r.size.h as f64 }
    }
    fn lerp(a: RectF, b: RectF, p: f64) -> RectF {
        let l = |x: f64, y: f64| x + (y - x) * p;
        RectF { x: l(a.x, b.x), y: l(a.y, b.y), w: l(a.w, b.w), h: l(a.h, b.h) }
    }
    /// The same middle at `f` of the size, at least 5 x 5 (Hyprland's popin).
    fn shrunk(self, f: f64) -> RectF {
        let (w, h) = ((self.w * f).max(5.0).min(self.w), (self.h * f).max(5.0).min(self.h));
        RectF { x: self.x + (self.w - w) / 2.0, y: self.y + (self.h - h) / 2.0, w, h }
    }
    /// In physical pixels of an output at `origin` and `scale`, the edges rounded (neighbours share their edge).
    pub fn physical(self, origin: Point<i32, Logical>, scale: f64) -> Rectangle<i32, Physical> {
        let x0 = ((self.x - origin.x as f64) * scale).round() as i32;
        let y0 = ((self.y - origin.y as f64) * scale).round() as i32;
        let x1 = ((self.x + self.w - origin.x as f64) * scale).round() as i32;
        let y1 = ((self.y + self.h - origin.y as f64) * scale).round() as i32;
        Rectangle::new((x0, y0).into(), ((x1 - x0).max(0), (y1 - y0).max(0)).into())
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum AnimKind {
    Open,
    Move,
}

#[derive(Clone, Copy, Debug)]
struct Anim {
    kind: AnimKind,
    start: Duration,
    from: RectF,
}

/// What the look keeps for each window (in the window's user data).
pub struct WinLook {
    /// the ids of its border and shadow pieces (the damage tracking tells pieces apart by them)
    ids: [Id; 16],
    /// where it was asked to be at the last frame, and where it was drawn
    goal: Option<Rectangle<i32, Logical>>,
    shown: Option<RectF>,
    anim: Option<Anim>,
}

impl Default for WinLook {
    fn default() -> Self {
        WinLook { ids: std::array::from_fn(|_| Id::new()), goal: None, shown: None, anim: None }
    }
}

/// Where a window is drawn this frame and how opaque, from where it should be (`goal`): an animation in between, or the goal.
/// `moved_by_hand`: the pointer drags or resizes it (no animation then, as on Hyprland).
pub fn place(
    look: &mut WinLook,
    goal: Rectangle<i32, Logical>,
    asked: bool,
    animate: bool,
    moved_by_hand: bool,
    now: Duration,
) -> (RectF, f32) {
    let target = RectF::of(goal);
    if !animate {
        look.anim = None;
    } else if look.goal.is_none() {
        // first drawn: it opens
        look.anim = Some(Anim { kind: AnimKind::Open, start: now, from: target.shrunk(0.0) });
    } else if look.goal != Some(goal) && !moved_by_hand {
        // moved, or a new size asked for by the compositor (a program changing its own size is not animated)
        let old = look.goal.unwrap_or(goal);
        if old.loc != goal.loc || asked {
            let from = look.shown.unwrap_or(target);
            // an opening one keeps opening, towards the new place
            let kind = look.anim.map(|a| a.kind).filter(|k| *k == AnimKind::Open).unwrap_or(AnimKind::Move);
            let start = if kind == AnimKind::Open { look.anim.map(|a| a.start).unwrap_or(now) } else { now };
            let from = if kind == AnimKind::Open { look.anim.map(|a| a.from).unwrap_or(from) } else { from };
            look.anim = Some(Anim { kind, start, from });
        }
    } else if moved_by_hand {
        look.anim = None;
    }
    look.goal = Some(goal);
    let (rect, alpha) = match look.anim {
        Some(a) => {
            let t = now.saturating_sub(a.start).as_secs_f64() / DURATION.as_secs_f64();
            if t >= 1.0 {
                look.anim = None;
                (target, 1.0)
            } else {
                let p = ease(t);
                match a.kind {
                    // grows from its middle as it fades in
                    AnimKind::Open => (RectF::lerp(target.shrunk(0.0), target, p), p as f32),
                    AnimKind::Move => (RectF::lerp(a.from, target, p), 1.0),
                }
            }
        }
        None => (target, 1.0),
    };
    look.shown = Some(rect);
    (rect, alpha)
}

/// Whether a window is in the middle of an animation.
pub fn animating(look: &WinLook) -> bool {
    look.anim.is_some()
}

/// Where a window was last drawn (for its closing animation).
pub fn shown(look: &WinLook) -> Option<RectF> {
    look.shown
}

/// A window that has closed, shrinking and fading out from its last picture (taken from the screen it was on).
pub struct Closing {
    pub id: Id,
    pub rect: RectF,
    pub start: Duration,
    /// the screen's pixels where it was (with its border), taken before the next frame; None until then
    pub picture: Option<(MemoryRenderBuffer, String)>,
}

/// Where a closing window is drawn now and how opaque; None once its animation is over.
pub fn closing_place(c: &Closing, now: Duration) -> Option<(RectF, f32)> {
    let t = now.saturating_sub(c.start).as_secs_f64() / DURATION.as_secs_f64();
    if t >= 1.0 {
        return None;
    }
    let p = ease(t);
    Some((RectF::lerp(c.rect, c.rect.shrunk(0.0), p), 1.0 - p as f32))
}

// ------------------------------------------------------------------ pictures: the border's corners and the shadow

/// The pictures of one scale and one set of colours.
pub struct Art {
    /// a number of its own (part of each piece's commit: a new picture is drawn when it changes)
    version: usize,
    border_px: i32,
    radius_px: i32,
    range_px: i32,
    /// top left, top right, bottom right, bottom left
    corners_active: [MemoryRenderBuffer; 4],
    corners_inactive: [MemoryRenderBuffer; 4],
    shadow_corners: [MemoryRenderBuffer; 4],
    /// top, right, bottom, left: 4 pixels long, stretched along the edge (the middle two used, so stretching blends nothing in)
    shadow_edges: [MemoryRenderBuffer; 4],
}

/// The pictures for an output's scale, made the first time they are needed and kept.
pub fn art(cache: &mut Vec<(u64, Look, Arc<Art>)>, look: &Look, scale: f64) -> Arc<Art> {
    let key = scale.to_bits();
    if let Some((_, _, a)) = cache.iter().find(|(k, l, _)| *k == key && l == look) {
        return a.clone();
    }
    static VERSION: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(1);
    let version = VERSION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let border_px = if look.border > 0 { ((look.border as f64 * scale).round() as i32).max(1) } else { 0 };
    let radius_px = (look.rounding as f64 * scale).round() as i32;
    let range_px = (look.shadow_range as f64 * scale).round() as i32;
    let outer = radius_px + border_px;
    let ring = |c: [u8; 4]| rotations(outer, outer, |x, y| ring_cover(x, y, outer as f64, radius_px as f64), c);
    let s = range_px + outer;
    let power = look.shadow_power;
    let shadow_at = move |d: f64| if d <= 0.0 || d >= range_px as f64 { 0.0 } else { (1.0 - d / range_px as f64).powi(power) };
    // the distance from the border's rounded outside, for the pixel's middle
    let corner = rotations(s, s, |x, y| shadow_at(((x - s as f64).hypot(y - s as f64)) - outer as f64), look.shadow);
    let edge = |w: i32, h: i32, along_x: bool, flip: bool| {
        picture(w, h, look.shadow, |x, y| {
            let p = if along_x { y } else { x };
            let d = if flip { p } else { range_px as f64 - p };
            shadow_at(d)
        })
    };
    let a = Arc::new(Art {
        version,
        border_px,
        radius_px,
        range_px,
        corners_active: ring(look.active),
        corners_inactive: ring(look.inactive),
        shadow_corners: corner,
        shadow_edges: [
            edge(4, range_px.max(1), true, false),
            edge(range_px.max(1), 4, false, true),
            edge(4, range_px.max(1), true, true),
            edge(range_px.max(1), 4, false, false),
        ],
    });
    // one set per output scale; pictures of colours no longer used go
    cache.retain(|(_, l, _)| l == look);
    cache.push((key, look.clone(), a.clone()));
    a
}

/// How much of the pixel whose middle is (x, y) a shape covers (16 samples), for a shape given by a test on a point.
fn cover(x: f64, y: f64, inside: impl Fn(f64, f64) -> bool) -> f64 {
    let mut n = 0;
    for i in 0..4 {
        for j in 0..4 {
            if inside(x - 0.5 + (i as f64 + 0.5) / 4.0, y - 0.5 + (j as f64 + 0.5) / 4.0) {
                n += 1;
            }
        }
    }
    n as f64 / 16.0
}

/// The border's corner: the ring between the window's rounding and the border's outside, around (outer, outer).
fn ring_cover(x: f64, y: f64, outer: f64, inner: f64) -> f64 {
    cover(x, y, |px, py| {
        let d = (px - outer).hypot(py - outer);
        d <= outer && d > inner
    })
}

/// A picture of `w` x `h` in one colour, each pixel at the share `f(x, y)` (of its middle) of the colour's alpha.
fn picture(w: i32, h: i32, c: [u8; 4], f: impl Fn(f64, f64) -> f64) -> MemoryRenderBuffer {
    let mut data = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let a = (f(x as f64 + 0.5, y as f64 + 0.5).clamp(0.0, 1.0) * c[3] as f64 / 255.0) as f32;
            // premultiplied, as Argb8888 bytes (B, G, R, A)
            let m = |v: u8| (v as f32 * a).round() as u8;
            data.extend_from_slice(&[m(c[2]), m(c[1]), m(c[0]), (a * 255.0).round() as u8]);
        }
    }
    MemoryRenderBuffer::from_slice(&data, Fourcc::Argb8888, (w.max(1), h.max(1)), 1, Transform::Normal, None)
}

/// The top-left corner's picture (drawn by `f` for the top-left), and the same turned for the other three corners.
fn rotations(w: i32, h: i32, f: impl Fn(f64, f64) -> f64, c: [u8; 4]) -> [MemoryRenderBuffer; 4] {
    let (wf, hf) = (w as f64, h as f64);
    [
        picture(w, h, c, &f),
        picture(w, h, c, |x, y| f(wf - x, y)),
        picture(w, h, c, |x, y| f(wf - x, hf - y)),
        picture(w, h, c, |x, y| f(x, hf - y)),
    ]
}

/// One picture of the compositor's at a place of its own in physical pixels (a corner, a stretched shadow edge, a closing
/// window's last picture): a memory buffer drawn with an id and a commit of its own, as the same picture is drawn many times.
pub struct Piece<R: Renderer> {
    id: Id,
    commit: CommitCounter,
    geo: Rectangle<i32, Physical>,
    inner: MemoryRenderBufferRenderElement<R>,
}

impl<R: Renderer> Element for Piece<R> {
    fn id(&self) -> &Id {
        &self.id
    }
    fn current_commit(&self) -> CommitCounter {
        self.commit
    }
    fn src(&self) -> Rectangle<f64, BufferCoords> {
        self.inner.src()
    }
    fn geometry(&self, _: Scale<f64>) -> Rectangle<i32, Physical> {
        self.geo
    }
    fn alpha(&self) -> f32 {
        self.inner.alpha()
    }
}

impl<R: Renderer + ImportMem> RenderElement<R> for Piece<R>
where
    R::TextureId: 'static,
{
    fn draw(
        &self,
        frame: &mut R::Frame<'_, '_>,
        src: Rectangle<f64, BufferCoords>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        _: &[Rectangle<i32, Physical>],
    ) -> Result<(), R::Error> {
        self.inner.draw(frame, src, dst, damage, &[])
    }
}

/// A piece from a picture: `crop` the part of it used (None: all), drawn over `geo`.
fn piece<R>(
    renderer: &mut R,
    id: &Id,
    commit: usize,
    buffer: &MemoryRenderBuffer,
    crop: Option<Rectangle<f64, Logical>>,
    geo: Rectangle<i32, Physical>,
    alpha: f32,
) -> Option<Piece<R>>
where
    R: Renderer + ImportMem,
    R::TextureId: Send + Clone + 'static,
{
    if geo.size.w <= 0 || geo.size.h <= 0 {
        return None;
    }
    let inner =
        MemoryRenderBufferRenderElement::from_buffer(renderer, geo.loc.to_f64(), buffer, Some(alpha), crop, None, Kind::Unspecified)
            .ok()?;
    Some(Piece { id: id.clone(), commit: CommitCounter::from(commit), geo, inner })
}

/// A solid strip (the border's straight parts).
pub struct Strip {
    id: Id,
    commit: CommitCounter,
    geo: Rectangle<i32, Physical>,
    color: Color32F,
}

impl Element for Strip {
    fn id(&self) -> &Id {
        &self.id
    }
    fn current_commit(&self) -> CommitCounter {
        self.commit
    }
    fn src(&self) -> Rectangle<f64, BufferCoords> {
        Rectangle::from_size((1.0, 1.0).into())
    }
    fn geometry(&self, _: Scale<f64>) -> Rectangle<i32, Physical> {
        self.geo
    }
}

impl<R: Renderer> RenderElement<R> for Strip {
    fn draw(
        &self,
        frame: &mut R::Frame<'_, '_>,
        _: Rectangle<f64, BufferCoords>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        _: &[Rectangle<i32, Physical>],
    ) -> Result<(), R::Error> {
        frame.draw_solid(dst, damage, self.color)
    }
}

/// What a window's look adds, in front-to-back order: `front` (the border) goes before the window's picture, `back` (the
/// shadow) after it.
pub struct Deco<R: Renderer> {
    pub front: Vec<Piece<R>>,
    pub strips: Vec<Strip>,
    pub back: Vec<Piece<R>>,
}

/// The border and shadow around a window drawn at `rect` (physical pixels on its output).
pub fn deco<R>(
    renderer: &mut R,
    art: &Art,
    look: &WinLook,
    rect: Rectangle<i32, Physical>,
    active: bool,
    alpha: f32,
    look_of: &Look,
) -> Deco<R>
where
    R: Renderer + ImportMem,
    R::TextureId: Send + Clone + 'static,
{
    let mut d = Deco { front: vec![], strips: vec![], back: vec![] };
    let (b, g) = (art.border_px, art.range_px);
    let outer = art.radius_px + b;
    // the border's outside
    let bx = Rectangle::<i32, Physical>::new((rect.loc.x - b, rect.loc.y - b).into(), (rect.size.w + 2 * b, rect.size.h + 2 * b).into());
    if bx.size.w < 2 * outer || bx.size.h < 2 * outer {
        return d;
    }
    let a8 = (alpha.clamp(0.0, 1.0) * 255.0).round() as usize;
    let commit = (art.version << 10) | ((active as usize) << 9) | a8;
    let (x0, y0, x1, y1) = (bx.loc.x, bx.loc.y, bx.loc.x + bx.size.w, bx.loc.y + bx.size.h);
    let r = |x: i32, y: i32, w: i32, h: i32| Rectangle::<i32, Physical>::new((x, y).into(), (w, h).into());
    let ids = &look.ids;
    if b > 0 {
        let corners = if active { &art.corners_active } else { &art.corners_inactive };
        let at = [
            r(x0, y0, outer, outer),
            r(x1 - outer, y0, outer, outer),
            r(x1 - outer, y1 - outer, outer, outer),
            r(x0, y1 - outer, outer, outer),
        ];
        for i in 0..4 {
            d.front.extend(piece(renderer, &ids[i], commit, &corners[i], None, at[i], alpha));
        }
        let c = if active { look_of.active } else { look_of.inactive };
        let a = c[3] as f32 / 255.0 * alpha;
        let color = Color32F::new(c[0] as f32 / 255.0 * a, c[1] as f32 / 255.0 * a, c[2] as f32 / 255.0 * a, a);
        let edges = [
            r(x0 + outer, y0, bx.size.w - 2 * outer, b),
            r(x1 - b, y0 + outer, b, bx.size.h - 2 * outer),
            r(x0 + outer, y1 - b, bx.size.w - 2 * outer, b),
            r(x0, y0 + outer, b, bx.size.h - 2 * outer),
        ];
        for (i, geo) in edges.into_iter().enumerate() {
            if geo.size.w > 0 && geo.size.h > 0 {
                d.strips.push(Strip { id: ids[4 + i].clone(), commit: CommitCounter::from(commit), geo, color });
            }
        }
    }
    if g > 0 {
        let s = g + outer;
        let at = [r(x0 - g, y0 - g, s, s), r(x1 - outer, y0 - g, s, s), r(x1 - outer, y1 - outer, s, s), r(x0 - g, y1 - outer, s, s)];
        for i in 0..4 {
            d.back.extend(piece(renderer, &ids[8 + i], commit, &art.shadow_corners[i], None, at[i], alpha));
        }
        let along: Rectangle<f64, Logical> = Rectangle::new((1.0, 0.0).into(), (2.0, g as f64).into());
        let across: Rectangle<f64, Logical> = Rectangle::new((0.0, 1.0).into(), (g as f64, 2.0).into());
        let edges = [
            (r(x0 + outer, y0 - g, bx.size.w - 2 * outer, g), along),
            (r(x1, y0 + outer, g, bx.size.h - 2 * outer), across),
            (r(x0 + outer, y1, bx.size.w - 2 * outer, g), along),
            (r(x0 - g, y0 + outer, g, bx.size.h - 2 * outer), across),
        ];
        for (i, (geo, crop)) in edges.into_iter().enumerate() {
            d.back.extend(piece(renderer, &ids[12 + i], commit, &art.shadow_edges[i], Some(crop), geo, alpha));
        }
    }
    d
}

/// A closing window's last picture, drawn shrinking and fading.
pub fn closing_piece<R>(
    renderer: &mut R,
    c: &Closing,
    buffer: &MemoryRenderBuffer,
    geo: Rectangle<i32, Physical>,
    alpha: f32,
) -> Option<Piece<R>>
where
    R: Renderer + ImportMem,
    R::TextureId: Send + Clone + 'static,
{
    let a8 = (alpha.clamp(0.0, 1.0) * 255.0).round() as usize;
    let mut p = piece(renderer, &c.id, a8, buffer, None, geo, alpha)?;
    // every frame of the animation is a change (the size and the alpha both move)
    p.commit = CommitCounter::from(a8 | ((geo.size.w as usize) << 8) | ((geo.size.h as usize) << 24));
    Some(p)
}

// ------------------------------------------------------------------ the rounded corners of the window's own picture

/// Which pixels of a corner of radius `r` the rounding keeps: per row from the outer edge, the first column kept whole, and the
/// columns before it that are partly inside with their share.
struct Corner {
    rows: Vec<(i32, Vec<(i32, f32)>)>,
}

fn corner(r: i32) -> Arc<Corner> {
    static CACHE: OnceLock<Mutex<HashMap<i32, Arc<Corner>>>> = OnceLock::new();
    let mut cache = CACHE.get_or_init(Default::default).lock().unwrap();
    cache
        .entry(r)
        .or_insert_with(|| {
            let rf = r as f64;
            let rows = (0..r)
                .map(|i| {
                    let mut partial = vec![];
                    let mut full = r;
                    for j in 0..r {
                        let c = cover(j as f64 + 0.5, i as f64 + 0.5, |x, y| (x - rf).hypot(y - rf) <= rf);
                        if c >= 1.0 {
                            full = j;
                            break;
                        }
                        if c > 0.0 {
                            partial.push((j, c as f32));
                        }
                    }
                    (full, partial)
                })
                .collect();
            Arc::new(Corner { rows })
        })
        .clone()
}

/// A part of a window's own picture (a surface of it, or of one of its menus), stretched about the window's corner while an
/// animation runs (`origin`, `rescale`), and the window's own surfaces clipped to its rounded rectangle (its corners cut,
/// nothing outside drawn: as on Hyprland, a program's own shadow outside its window geometry is not shown). Menus are not
/// clipped.
pub struct Clipped<R: Renderer> {
    inner: WaylandSurfaceRenderElement<R>,
    /// the window's rectangle on the output (physical pixels), the corners' radius and which of their pixels are kept
    clip: Option<(Rectangle<i32, Physical>, i32, Arc<Corner>)>,
    origin: Point<i32, Physical>,
    rescale: Scale<f64>,
}

impl<R: Renderer + ImportAll> Clipped<R> {
    pub fn new(
        inner: WaylandSurfaceRenderElement<R>,
        clip: Option<(Rectangle<i32, Physical>, i32)>,
        origin: Point<i32, Physical>,
        rescale: Scale<f64>,
    ) -> Self {
        let clip = clip.map(|(c, radius)| {
            let radius = radius.min(c.size.w / 2).min(c.size.h / 2).max(0);
            (c, radius, corner(radius))
        });
        Clipped { inner, clip, origin, rescale }
    }

    fn stretched(&self) -> bool {
        self.rescale != Scale::from(1.0)
    }

    /// The parts kept whole: the middle band, and in each corner row the span between the curves.
    fn whole(c: Rectangle<i32, Physical>, r: i32, corner: &Corner) -> Vec<Rectangle<i32, Physical>> {
        let mut v = vec![Rectangle::new((c.loc.x, c.loc.y + r).into(), (c.size.w, c.size.h - 2 * r).into())];
        for (i, (full, _)) in corner.rows.iter().enumerate() {
            let w = c.size.w - 2 * full;
            if w > 0 {
                v.push(Rectangle::new((c.loc.x + full, c.loc.y + i as i32).into(), (w, 1).into()));
                v.push(Rectangle::new((c.loc.x + full, c.loc.y + c.size.h - 1 - i as i32).into(), (w, 1).into()));
            }
        }
        v
    }

    /// The pixels on the curves, with their share.
    fn edge_pixels(c: Rectangle<i32, Physical>, corner: &Corner) -> impl Iterator<Item = (Point<i32, Physical>, f32)> + '_ {
        corner.rows.iter().enumerate().flat_map(move |(i, (_, partial))| {
            let i = i as i32;
            partial.iter().flat_map(move |(j, cov)| {
                let (l, r) = (c.loc.x + j, c.loc.x + c.size.w - 1 - j);
                let (t, b) = (c.loc.y + i, c.loc.y + c.size.h - 1 - i);
                [((l, t), *cov), ((r, t), *cov), ((r, b), *cov), ((l, b), *cov)].map(|(p, v)| (Point::from(p), v))
            })
        })
    }
}

impl<R: Renderer + ImportAll> Element for Clipped<R> {
    fn id(&self) -> &Id {
        self.inner.id()
    }
    fn current_commit(&self) -> CommitCounter {
        self.inner.current_commit()
    }
    fn src(&self) -> Rectangle<f64, BufferCoords> {
        self.inner.src()
    }
    fn transform(&self) -> Transform {
        self.inner.transform()
    }
    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        let g = self.inner.geometry(scale);
        if !self.stretched() {
            return g;
        }
        // both edges scaled about the window's corner, then rounded (neighbouring surfaces keep sharing an edge)
        let (o, k) = (self.origin, self.rescale);
        let x0 = o.x + ((g.loc.x - o.x) as f64 * k.x).round() as i32;
        let y0 = o.y + ((g.loc.y - o.y) as f64 * k.y).round() as i32;
        let x1 = o.x + ((g.loc.x + g.size.w - o.x) as f64 * k.x).round() as i32;
        let y1 = o.y + ((g.loc.y + g.size.h - o.y) as f64 * k.y).round() as i32;
        Rectangle::new((x0, y0).into(), (x1 - x0, y1 - y0).into())
    }
    fn damage_since(&self, scale: Scale<f64>, commit: Option<CommitCounter>) -> DamageSet<i32, Physical> {
        if self.stretched() {
            // only while an animation runs, when it is drawn anew each frame anyway
            return DamageSet::from_slice(&[Rectangle::from_size(self.geometry(scale).size)]);
        }
        self.inner.damage_since(scale, commit)
    }
    fn opaque_regions(&self, scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        let Some((c, r, _)) = &self.clip else {
            return if self.stretched() { OpaqueRegions::default() } else { self.inner.opaque_regions(scale) };
        };
        if self.stretched() {
            return OpaqueRegions::default();
        }
        // only what is drawn whole and covers its pixels: the opaque parts inside the rounded rectangle, corners left out
        let at = self.inner.geometry(scale).loc;
        let (c, r) = (*c, *r);
        let core = [
            Rectangle::<i32, Physical>::new((c.loc.x, c.loc.y + r).into(), (c.size.w, c.size.h - 2 * r).into()),
            Rectangle::new((c.loc.x + r, c.loc.y).into(), (c.size.w - 2 * r, r).into()),
            Rectangle::new((c.loc.x + r, c.loc.y + c.size.h - r).into(), (c.size.w - 2 * r, r).into()),
        ];
        let mut out = vec![];
        for o in self.inner.opaque_regions(scale).iter() {
            let abs = Rectangle::new(o.loc + at, o.size);
            for k in &core {
                if let Some(i) = abs.intersection(*k) {
                    out.push(Rectangle::new(i.loc - at, i.size));
                }
            }
        }
        OpaqueRegions::from_slice(&out)
    }
    fn alpha(&self) -> f32 {
        self.inner.alpha()
    }
    fn kind(&self) -> Kind {
        self.inner.kind()
    }
}

impl<R: Renderer + ImportAll> RenderElement<R> for Clipped<R>
where
    R::TextureId: Texture + 'static,
{
    fn draw(
        &self,
        frame: &mut R::Frame<'_, '_>,
        src: Rectangle<f64, BufferCoords>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), R::Error> {
        let Some((c, r, corner)) = &self.clip else {
            return self.inner.draw(frame, src, dst, damage, opaque_regions);
        };
        let damaged = |p: Point<i32, Physical>| damage.iter().any(|d| Rectangle::new(d.loc + dst.loc, d.size).contains(p));
        // a picture without alpha cannot be drawn at a share (pixman copies it): its curve pixels go whole or not at all
        let blends = match self.inner.texture() {
            WaylandSurfaceTexture::Texture(t) => t.format().is_none_or(smithay::backend::allocator::format::has_alpha),
            WaylandSurfaceTexture::SolidColor(_) => true,
        };
        let mut kept: Vec<Rectangle<i32, Physical>> = vec![];
        let whole = Self::whole(*c, *r, corner);
        for d in damage {
            let abs = Rectangle::new(d.loc + dst.loc, d.size);
            for w in &whole {
                if let Some(i) = abs.intersection(*w) {
                    kept.push(Rectangle::new(i.loc - dst.loc, i.size));
                }
            }
        }
        let mut shares: Vec<(Rectangle<i32, Physical>, f32)> = vec![];
        for (p, cov) in Self::edge_pixels(*c, corner) {
            if !damaged(p) || !dst.contains(p) {
                continue;
            }
            let one = Rectangle::new(p - dst.loc, (1, 1).into());
            if !blends {
                if cov >= 0.5 {
                    kept.push(one);
                }
            } else {
                shares.push((one, cov));
            }
        }
        if !kept.is_empty() {
            self.inner.draw(frame, src, dst, &kept, opaque_regions)?;
        }
        let alpha = self.inner.alpha();
        for (one, cov) in shares {
            match self.inner.texture() {
                WaylandSurfaceTexture::Texture(t) => {
                    frame.render_texture_from_to(t, src, dst, &[one], &[], self.inner.transform(), alpha * cov)?
                }
                WaylandSurfaceTexture::SolidColor(c) => frame.draw_solid(dst, &[one], *c * (alpha * cov))?,
            }
        }
        Ok(())
    }
}

/// The clip a window's picture gets on an output: its rectangle in physical pixels, and the rounding (none in full screen).
pub fn clip_of(rect: RectF, origin: Point<i32, Logical>, scale: f64, look: &Look, fullscreen: bool) -> (Rectangle<i32, Physical>, i32) {
    let radius = if fullscreen { 0 } else { (look.rounding as f64 * scale).round() as i32 };
    (rect.physical(origin, scale), radius)
}

/// The scale to draw a window's picture at so that its `size` fills `rect` (an animation stretches it, as Hyprland does).
pub fn stretch(rect: RectF, size: Size<i32, Logical>) -> (f64, f64) {
    if size.w <= 0 || size.h <= 0 {
        return (1.0, 1.0);
    }
    (rect.w / size.w as f64, rect.h / size.h as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hyprlands_default_curve() {
        assert_eq!(ease(0.0), 0.0);
        assert!((ease(1.0) - 1.0).abs() < 1e-9);
        // quick at first, then easing out (0, 0.75, 0.15, 1)
        assert!(ease(0.1) > 0.45, "{}", ease(0.1));
        assert!(ease(0.5) > 0.95 && ease(0.5) < 1.0, "{}", ease(0.5));
        let mut last = 0.0;
        for i in 0..=100 {
            let v = ease(i as f64 / 100.0);
            assert!(v >= last - 1e-9);
            last = v;
        }
    }

    #[test]
    fn the_sessions_values() {
        let l = Look::new(None, true, false);
        assert_eq!((l.gaps_in, l.gaps_out, l.border, l.rounding, l.shadow_range, l.shadow_power), (4, 8, 1, 10, 18, 3));
        assert_eq!(l.active, [0x7c, 0x54, 0xd6, 0xcc], "Yggdrasil's purple at cc");
        assert_eq!(l.inactive, [255, 255, 255, 0x14]);
        assert_eq!(Look::new(Some([1, 2, 3]), false, true).active, [1, 2, 3, 0xcc]);
        assert_eq!(Look::new(None, false, false).inactive, [0, 0, 0, 0x18]);
        assert_eq!(l.shadow, [0, 0, 0, 0x44]);
        assert!(l.animate && !Look::new(None, true, true).animate);
    }

    #[test]
    fn a_window_opens_moves_and_settles() {
        let mut w = WinLook::default();
        let goal = Rectangle::new((100, 100).into(), (400, 300).into());
        let ms = Duration::from_millis;
        // first drawn: from its middle, transparent
        let (r, a) = place(&mut w, goal, false, true, false, ms(0));
        assert_eq!((r.w, r.h, a), (5.0, 5.0, 0.0));
        assert_eq!((r.x + r.w / 2.0, r.y + r.h / 2.0), (300.0, 250.0));
        let (r, a) = place(&mut w, goal, false, true, false, ms(200));
        assert!(r.w > 200.0 && r.w < 400.0 && a > 0.5 && a < 1.0, "{r:?} {a}");
        let (r, a) = place(&mut w, goal, false, true, false, ms(800));
        assert_eq!((r, a), (RectF::of(goal), 1.0));
        assert!(!animating(&w));
        // moved: slides from where it was
        let to = Rectangle::new((0, 0).into(), (800, 600).into());
        let (r, _) = place(&mut w, to, true, true, false, ms(1000));
        assert_eq!(r, RectF::of(goal));
        let (r, _) = place(&mut w, to, true, true, false, ms(1100));
        assert!(r.x < 100.0 && r.x > 0.0 && r.w > 400.0 && r.w < 800.0, "{r:?}");
        assert!(animating(&w));
        // dragged by hand: no animation
        let by_hand = Rectangle::new((10, 10).into(), (800, 600).into());
        assert_eq!(place(&mut w, by_hand, false, true, true, ms(1150)).0, RectF::of(by_hand));
        // a program changing its own size where it is: no animation either
        let own = Rectangle::new((10, 10).into(), (820, 600).into());
        assert_eq!(place(&mut w, own, false, true, false, ms(1200)).0, RectF::of(own));
        // reduce motion: always where it should be
        let mut w = WinLook::default();
        assert_eq!(place(&mut w, goal, false, false, false, ms(0)), (RectF::of(goal), 1.0));
    }

    #[test]
    fn a_closing_window_shrinks_and_fades() {
        let c = Closing {
            id: Id::new(),
            rect: RectF::of(Rectangle::new((0, 0).into(), (100, 80).into())),
            start: Duration::ZERO,
            picture: None,
        };
        // the curve starts steep: a tenth of the way after the first millisecond
        let (r, a) = closing_place(&c, Duration::ZERO).unwrap();
        assert_eq!((r.w, a), (100.0, 1.0));
        let (r, a) = closing_place(&c, Duration::from_millis(1)).unwrap();
        assert!(r.w > 85.0 && a > 0.85, "{r:?} {a}");
        let (r, a) = closing_place(&c, Duration::from_millis(400)).unwrap();
        assert!(r.w < 20.0 && a < 0.05, "{r:?} {a}");
        assert!(closing_place(&c, Duration::from_millis(800)).is_none());
    }

    #[test]
    fn the_corner_keeps_whole_pixels_inside_the_curve() {
        let c = corner(10);
        assert_eq!(c.rows.len(), 10);
        // the outermost row starts well in, the innermost almost at the edge
        assert!(c.rows[0].0 >= 4 && c.rows[9].0 <= 1, "{:?}", c.rows.iter().map(|r| r.0).collect::<Vec<_>>());
        for (full, partial) in &c.rows {
            assert!(partial.iter().all(|(j, v)| *j < *full && *v > 0.0 && *v < 1.0));
        }
        assert!(corner(0).rows.is_empty());
    }

    #[test]
    fn rectangles_round_to_shared_edges() {
        let r = RectF { x: 10.4, y: 0.0, w: 100.3, h: 50.0 };
        let p = r.physical((0, 0).into(), 1.5);
        assert_eq!((p.loc.x, p.size.w), (16, 166 - 16));
        assert_eq!(stretch(RectF { x: 0.0, y: 0.0, w: 200.0, h: 50.0 }, (100, 100).into()), (2.0, 0.5));
    }
}
