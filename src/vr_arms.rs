//! VR arms: a character's own forearms and hands cut from its skinned body, for a VR host to
//! draw on the player's controllers in place of the body it hides.
//!
//! One flow for every character and skin, whatever its game's model format:
//!
//! 1. The owner's adapter lays the body out as a [`Mesh`]: joints (parents and bind-pose world
//!    matrices), each vertex's bind-pose position and joint weights, and triangles, in the same
//!    vertex and triangle order its view draws the body.
//! 2. [`cut`] keeps each triangle whose three corners all hang mostly (at least half) from a
//!    side's forearm joint or below it, and finds that side's arm space ([`Arm::frame`]).
//! 3. [`Arms::save`] writes the result into the extracted asset cache (`vr_arms/<name>.arms`,
//!    plain text) on first run, beside the other derived files; [`Arms::load`] reads it back,
//!    and a cut made for another vertex or triangle count is rejected.
//! 4. Each frame the view draws a side's kept triangles from the body's own skinned vertices,
//!    taken into arm space by [`Arm::to_arm`] with the wrist joint's current matrix, and the
//!    host puts arm space on the controller.
//!
//! [`capture`] renders a cut to a picture for checking it in a viewer.
//!
//! Arm space: the origin at the middle of the hand (its vertices' centre in the bind pose), +z
//! along the forearm toward the fingers (elbow to wrist), +y the bind pose's up made square to
//! it, and +x = y × z. The forearm hangs back along -z. A host that mirrors X (ModelView does)
//! sees x right, y up and z forward, which are a VR controller's own axes.
//!
//! Matrices are column-major `[f32; 16]` (glam's `to_cols_array`). Positions are in the
//! model's own units and space; nothing here knows a game's scale or axes.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// The `.arms` format's version; a cache written by another one is rebuilt.
pub const VERSION: u32 = 1;

/// The folder under an asset cache that holds the cuts.
pub const DIR: &str = "vr_arms";

/// A vertex counts as on the arm when this much of its weight hangs from the arm's joints.
pub const MIN_WEIGHT: f32 = 0.5;

/// [`Arm::vertex_map`]'s mark for a vertex nowhere near the arm.
pub const COLLAPSED: u32 = u32::MAX;

/// A skinned body as its view draws it.
#[derive(Clone, Debug, Default)]
pub struct Mesh {
    /// Each joint's parent.
    pub parents: Vec<Option<usize>>,
    /// Each joint's bind-pose world matrix, in the vertices' space.
    pub bind: Vec<[f32; 16]>,
    /// Each vertex's bind-pose position.
    pub positions: Vec<[f32; 3]>,
    /// Each vertex's joints and weights.
    pub weights: Vec<Vec<(u16, f32)>>,
    /// Triangles, as vertex indices.
    pub triangles: Vec<[u32; 3]>,
}

impl Mesh {
    /// `root` and every joint below it.
    pub fn subtree(&self, root: usize) -> Vec<bool> {
        let mut set = vec![false; self.parents.len()];
        for (j, s) in set.iter_mut().enumerate() {
            let mut at = Some(j);
            let mut guard = 0;
            while let Some(i) = at {
                if i == root {
                    *s = true;
                    break;
                }
                guard += 1;
                if guard > self.parents.len() {
                    break;
                }
                at = self.parents.get(i).copied().flatten();
            }
        }
        set
    }

    /// How much of vertex `v` hangs from the joints in `set`.
    pub fn weight_in(&self, v: usize, set: &[bool]) -> f32 {
        self.weights
            .get(v)
            .map_or(0.0, |ws| ws.iter().filter(|(j, _)| set.get(*j as usize).copied().unwrap_or(false)).map(|(_, w)| *w).sum())
    }
}

/// Which joints make one side's arm: the forearm's own joint (the elbow) and the hand's (the
/// wrist). Everything from the elbow down is the arm.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Side {
    pub elbow: usize,
    pub wrist: usize,
}

impl Side {
    /// The wrist and its parent as the elbow, the usual rig (a forearm joint holding the hand).
    pub fn from_wrist(mesh: &Mesh, wrist: usize) -> Option<Side> {
        let elbow = mesh.parents.get(wrist).copied().flatten()?;
        Some(Side { elbow, wrist })
    }

    /// A rig whose whole arm is one joint off the body (no forearm joint: Pikmin's captains).
    /// The arm is that joint's subtree; arm space's +z runs from the joint out to its vertices.
    pub fn limb(joint: usize) -> Side {
        Side { elbow: joint, wrist: joint }
    }
}

/// One side's cut.
#[derive(Clone, Debug, PartialEq)]
pub struct Arm {
    /// 0 the character's right arm, 1 its left.
    pub side: u8,
    pub elbow: u32,
    pub wrist: u32,
    /// Arm space into the wrist joint's own space (see the module docs).
    pub frame: [f32; 16],
    /// The kept triangles' indices, ascending.
    pub triangles: Vec<u32>,
}

impl Arm {
    /// From the body's space into arm space, with the wrist joint at `wrist_now` this frame
    /// (its posed world matrix, in the same space the view skins the body into).
    pub fn to_arm(&self, wrist_now: &[f32; 16]) -> [f32; 16] {
        inverse(&mul(wrist_now, &self.frame))
    }

    /// Where each of a mesh's `vertices` comes from in the arm's draw, for a view that draws the
    /// arm from the body's own vertex buffer and index list (an indexed mesh shares a vertex
    /// between kept and dropped triangles): a kept triangle's vertex is itself; any other next
    /// to one is snapped onto a kept neighbour, so the seam's dropped triangles close up rather
    /// than stretch; the rest are [`COLLAPSED`] (draw them at the origin, degenerate).
    pub fn vertex_map(&self, triangles: &[[u32; 3]], vertices: usize) -> Vec<u32> {
        let mut map = vec![COLLAPSED; vertices];
        for &t in &self.triangles {
            if let Some(tri) = triangles.get(t as usize) {
                for &v in tri {
                    if let Some(m) = map.get_mut(v as usize) {
                        *m = v;
                    }
                }
            }
        }
        let kept = map.clone();
        let is_kept = |v: u32| kept.get(v as usize).is_some_and(|&m| m != COLLAPSED);
        for tri in triangles {
            let Some(&anchor) = tri.iter().find(|&&v| is_kept(v)) else { continue };
            for &v in tri {
                if let Some(m) = map.get_mut(v as usize)
                    && *m == COLLAPSED
                {
                    *m = anchor;
                }
            }
        }
        map
    }

    /// For each of `count` triangles, whether it's kept.
    pub fn mask(&self, count: usize) -> Vec<bool> {
        let mut keep = vec![false; count];
        for &t in &self.triangles {
            if let Some(k) = keep.get_mut(t as usize) {
                *k = true;
            }
        }
        keep
    }
}

/// A body's cut: up to one [`Arm`] a side.
#[derive(Clone, Debug, PartialEq)]
pub struct Arms {
    /// The cache name (`vr_arms/<name>.arms`): character, skin and costume.
    pub name: String,
    /// The mesh it was cut from, to reject a cut made for another layout.
    pub vertices: u32,
    pub triangle_count: u32,
    pub arms: Vec<Arm>,
}

impl Arms {
    /// The side's cut, if it has one.
    pub fn side(&self, side: u8) -> Option<&Arm> {
        self.arms.iter().find(|a| a.side == side)
    }

    /// Whether this cut fits a mesh with these counts.
    pub fn fits(&self, vertices: usize, triangles: usize) -> bool {
        self.vertices as usize == vertices && self.triangle_count as usize == triangles
    }

    /// The `.arms` text.
    pub fn to_text(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(s, "vr-arms {VERSION}");
        let _ = writeln!(s, "name {}", self.name);
        let _ = writeln!(s, "mesh {} {}", self.vertices, self.triangle_count);
        for a in &self.arms {
            let _ = writeln!(s, "arm {} {} {}", a.side, a.elbow, a.wrist);
            let _ = write!(s, "frame");
            for f in a.frame {
                let _ = write!(s, " {f:?}");
            }
            let _ = writeln!(s);
            // Runs of kept triangles: start and length pairs.
            let runs = runs(&a.triangles);
            let _ = write!(s, "keep {}", runs.len());
            for (start, len) in runs {
                let _ = write!(s, " {start} {len}");
            }
            let _ = writeln!(s);
        }
        s
    }

    /// Reads `.arms` text; None for another version or anything malformed.
    pub fn parse(text: &str) -> Option<Arms> {
        let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
        let mut head = lines.next()?.split_whitespace();
        if head.next()? != "vr-arms" || head.next()?.parse::<u32>().ok()? != VERSION {
            return None;
        }
        let name = lines.next()?.strip_prefix("name ")?.to_owned();
        let mut mesh = lines.next()?.split_whitespace();
        if mesh.next()? != "mesh" {
            return None;
        }
        let vertices = mesh.next()?.parse().ok()?;
        let triangle_count = mesh.next()?.parse().ok()?;
        let mut arms = Vec::new();
        while let Some(line) = lines.next() {
            let mut w = line.split_whitespace();
            if w.next()? != "arm" {
                return None;
            }
            let side = w.next()?.parse().ok()?;
            let elbow = w.next()?.parse().ok()?;
            let wrist = w.next()?.parse().ok()?;
            let mut f = lines.next()?.split_whitespace();
            if f.next()? != "frame" {
                return None;
            }
            let mut frame = [0.0f32; 16];
            for x in &mut frame {
                *x = f.next()?.parse().ok()?;
            }
            let mut k = lines.next()?.split_whitespace();
            if k.next()? != "keep" {
                return None;
            }
            let n: usize = k.next()?.parse().ok()?;
            let mut triangles = Vec::new();
            for _ in 0..n {
                let start: u32 = k.next()?.parse().ok()?;
                let len: u32 = k.next()?.parse().ok()?;
                if start.checked_add(len)? > triangle_count {
                    return None;
                }
                triangles.extend(start..start + len);
            }
            arms.push(Arm { side, elbow, wrist, frame, triangles });
        }
        Some(Arms { name, vertices, triangle_count, arms })
    }

    /// `<assets>/vr_arms/<name>.arms`.
    pub fn path(assets: &Path, name: &str) -> PathBuf {
        assets.join(DIR).join(format!("{name}.arms"))
    }

    /// Writes the cut into the asset cache.
    pub fn save(&self, assets: &Path) -> std::io::Result<PathBuf> {
        let path = Self::path(assets, &self.name);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&path, self.to_text())?;
        Ok(path)
    }

    /// The cached cut called `name`, if there is one in this version.
    pub fn load(assets: &Path, name: &str) -> Option<Arms> {
        Arms::parse(&std::fs::read_to_string(Self::path(assets, name)).ok()?)
    }
}

fn runs(sorted: &[u32]) -> Vec<(u32, u32)> {
    let mut out: Vec<(u32, u32)> = Vec::new();
    for &t in sorted {
        match out.last_mut() {
            Some((start, len)) if *start + *len == t => *len += 1,
            _ => out.push((t, 1)),
        }
    }
    out
}

/// Cuts both arms from `mesh`: `sides[0]` the right, `sides[1]` the left (None: no arm).
pub fn cut(name: &str, mesh: &Mesh, sides: [Option<Side>; 2]) -> Arms {
    let mut arms = Vec::new();
    for (i, side) in sides.iter().enumerate() {
        let Some(side) = *side else { continue };
        if side.elbow >= mesh.parents.len() || side.wrist >= mesh.parents.len() {
            continue;
        }
        let Some(frame) = frame(mesh, side) else { continue };
        let arm = mesh.subtree(side.elbow);
        let triangles = mesh
            .triangles
            .iter()
            .enumerate()
            .filter(|(_, t)| t.iter().all(|&v| mesh.weight_in(v as usize, &arm) >= MIN_WEIGHT))
            .map(|(i, _)| i as u32)
            .collect();
        arms.push(Arm { side: i as u8, elbow: side.elbow as u32, wrist: side.wrist as u32, frame, triangles });
    }
    Arms {
        name: name.to_owned(),
        vertices: mesh.positions.len() as u32,
        triangle_count: mesh.triangles.len() as u32,
        arms,
    }
}

// The side's arm space as a matrix into the wrist joint's own space.
fn frame(mesh: &Mesh, side: Side) -> Option<[f32; 16]> {
    let wrist = mesh.bind.get(side.wrist)?;
    let elbow = mesh.bind.get(side.elbow)?;
    let wrist_at = [wrist[12], wrist[13], wrist[14]];
    let elbow_at = [elbow[12], elbow[13], elbow[14]];
    let hand = mesh.subtree(side.wrist);
    let (mut sum, mut n) = ([0.0f32; 3], 0u32);
    for v in 0..mesh.positions.len() {
        if mesh.weight_in(v, &hand) >= MIN_WEIGHT {
            sum = add(sum, mesh.positions[v]);
            n += 1;
        }
    }
    let centre = if n > 0 { scale(sum, 1.0 / n as f32) } else { wrist_at };
    // A one-joint limb (a stubby arm on a single bone: Side::limb) points from its joint out to
    // its own vertices.
    let forward = if side.elbow == side.wrist { normalize(sub(centre, wrist_at))? } else { normalize(sub(wrist_at, elbow_at))? };
    let up = normalize(sub([0.0, 1.0, 0.0], scale(forward, forward[1])))
        .or_else(|| normalize(sub([0.0, 0.0, 1.0], scale(forward, forward[2]))))?;
    let right = cross(up, forward);
    let model = [
        right[0], right[1], right[2], 0.0, up[0], up[1], up[2], 0.0, forward[0], forward[1], forward[2], 0.0, centre[0],
        centre[1], centre[2], 1.0,
    ];
    Some(mul(&inverse(wrist), &model))
}

// ---- small column-major matrix and vector maths ------------------------------------------------

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn scale(a: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn normalize(a: [f32; 3]) -> Option<[f32; 3]> {
    let l = dot(a, a).sqrt();
    (l > 1e-6 && l.is_finite()).then(|| scale(a, 1.0 / l))
}

/// `a * b`, column-major.
pub fn mul(a: &[f32; 16], b: &[f32; 16]) -> [f32; 16] {
    let mut out = [0.0f32; 16];
    for c in 0..4 {
        for r in 0..4 {
            out[c * 4 + r] = (0..4).map(|k| a[k * 4 + r] * b[c * 4 + k]).sum();
        }
    }
    out
}

/// A point through a column-major matrix.
pub fn transform_point(m: &[f32; 16], p: [f32; 3]) -> [f32; 3] {
    [
        m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12],
        m[1] * p[0] + m[5] * p[1] + m[9] * p[2] + m[13],
        m[2] * p[0] + m[6] * p[1] + m[10] * p[2] + m[14],
    ]
}

/// The general 4×4 inverse (identity for a singular matrix).
pub fn inverse(m: &[f32; 16]) -> [f32; 16] {
    let mut inv = [0.0f32; 16];
    inv[0] = m[5] * m[10] * m[15] - m[5] * m[11] * m[14] - m[9] * m[6] * m[15] + m[9] * m[7] * m[14] + m[13] * m[6] * m[11]
        - m[13] * m[7] * m[10];
    inv[4] = -m[4] * m[10] * m[15] + m[4] * m[11] * m[14] + m[8] * m[6] * m[15] - m[8] * m[7] * m[14] - m[12] * m[6] * m[11]
        + m[12] * m[7] * m[10];
    inv[8] = m[4] * m[9] * m[15] - m[4] * m[11] * m[13] - m[8] * m[5] * m[15] + m[8] * m[7] * m[13] + m[12] * m[5] * m[11]
        - m[12] * m[7] * m[9];
    inv[12] = -m[4] * m[9] * m[14] + m[4] * m[10] * m[13] + m[8] * m[5] * m[14] - m[8] * m[6] * m[13] - m[12] * m[5] * m[10]
        + m[12] * m[6] * m[9];
    inv[1] = -m[1] * m[10] * m[15] + m[1] * m[11] * m[14] + m[9] * m[2] * m[15] - m[9] * m[3] * m[14] - m[13] * m[2] * m[11]
        + m[13] * m[3] * m[10];
    inv[5] = m[0] * m[10] * m[15] - m[0] * m[11] * m[14] - m[8] * m[2] * m[15] + m[8] * m[3] * m[14] + m[12] * m[2] * m[11]
        - m[12] * m[3] * m[10];
    inv[9] = -m[0] * m[9] * m[15] + m[0] * m[11] * m[13] + m[8] * m[1] * m[15] - m[8] * m[3] * m[13] - m[12] * m[1] * m[11]
        + m[12] * m[3] * m[9];
    inv[13] = m[0] * m[9] * m[14] - m[0] * m[10] * m[13] - m[8] * m[1] * m[14] + m[8] * m[2] * m[13] + m[12] * m[1] * m[10]
        - m[12] * m[2] * m[9];
    inv[2] = m[1] * m[6] * m[15] - m[1] * m[7] * m[14] - m[5] * m[2] * m[15] + m[5] * m[3] * m[14] + m[13] * m[2] * m[7]
        - m[13] * m[3] * m[6];
    inv[6] = -m[0] * m[6] * m[15] + m[0] * m[7] * m[14] + m[4] * m[2] * m[15] - m[4] * m[3] * m[14] - m[12] * m[2] * m[7]
        + m[12] * m[3] * m[6];
    inv[10] = m[0] * m[5] * m[15] - m[0] * m[7] * m[13] - m[4] * m[1] * m[15] + m[4] * m[3] * m[13] + m[12] * m[1] * m[7]
        - m[12] * m[3] * m[5];
    inv[14] = -m[0] * m[5] * m[14] + m[0] * m[6] * m[13] + m[4] * m[1] * m[14] - m[4] * m[2] * m[13] - m[12] * m[1] * m[6]
        + m[12] * m[2] * m[5];
    inv[3] = -m[1] * m[6] * m[11] + m[1] * m[7] * m[10] + m[5] * m[2] * m[11] - m[5] * m[3] * m[10] - m[9] * m[2] * m[7]
        + m[9] * m[3] * m[6];
    inv[7] = m[0] * m[6] * m[11] - m[0] * m[7] * m[10] - m[4] * m[2] * m[11] + m[4] * m[3] * m[10] + m[8] * m[2] * m[7]
        - m[8] * m[3] * m[6];
    inv[11] = -m[0] * m[5] * m[11] + m[0] * m[7] * m[9] + m[4] * m[1] * m[11] - m[4] * m[3] * m[9] - m[8] * m[1] * m[7]
        + m[8] * m[3] * m[5];
    inv[15] = m[0] * m[5] * m[10] - m[0] * m[6] * m[9] - m[4] * m[1] * m[10] + m[4] * m[2] * m[9] + m[8] * m[1] * m[6]
        - m[8] * m[2] * m[5];
    let det = m[0] * inv[0] + m[1] * inv[4] + m[2] * inv[8] + m[3] * inv[12];
    if det.abs() < 1e-20 || !det.is_finite() {
        let mut id = [0.0f32; 16];
        id[0] = 1.0;
        id[5] = 1.0;
        id[10] = 1.0;
        id[15] = 1.0;
        return id;
    }
    for x in &mut inv {
        *x /= det;
    }
    inv
}

// ---- the proof picture ----------------------------------------------------------------------

/// A cut as a picture, 4 panels across: the body from the front and from its right side in the
/// bind pose (grey, the right arm red, the left blue), then each arm alone in its arm space
/// from the side (+z right, +y up, the axes through the origin; the hand ahead of it, the
/// forearm behind). `label` is lettered across the top. RGBA8, rows top first.
pub fn capture(mesh: &Mesh, arms: &Arms, label: &str) -> (u32, u32, Vec<u8>) {
    const P: usize = 256;
    const TOP: usize = 20;
    let (w, h) = (P * 4, P + TOP);
    let mut c = Canvas::new(w, h);
    c.text(4, 6, label, [235, 235, 235]);
    let tri_side: Vec<u8> = {
        let mut s = vec![2u8; mesh.triangles.len()];
        for a in &arms.arms {
            for &t in &a.triangles {
                if let Some(x) = s.get_mut(t as usize) {
                    *x = a.side;
                }
            }
        }
        s
    };
    let colour = |s: u8| match s {
        0 => [220, 60, 50],
        1 => [60, 110, 230],
        _ => [150, 150, 150],
    };
    // The body: front (x, y) and side (z, y).
    for (panel, axes) in [(0usize, (0usize, false)), (1, (2, true))] {
        let pts: Vec<[f32; 3]> = mesh.positions.clone();
        let view = |p: [f32; 3]| -> [f32; 3] {
            let (a, flip) = axes;
            let x = if flip { -p[a] } else { p[a] };
            [x, p[1], if a == 0 { -p[2] } else { p[0] }]
        };
        let used: Vec<[f32; 3]> = mesh.triangles.iter().flat_map(|t| t.iter().map(|&v| view(pts[v as usize]))).collect();
        let fit = Fit::new(&used, P, TOP, panel * P);
        for (i, t) in mesh.triangles.iter().enumerate() {
            let [a, b, d] = t.map(|v| fit.px(view(pts[v as usize])));
            c.tri(a, b, d, shade(colour(tri_side[i]), mesh, t, &pts, axes.0));
        }
    }
    // Each arm alone in arm space, from the side: (z, y), +z right.
    for side in 0..2u8 {
        let x0 = (2 + side as usize) * P;
        let Some(arm) = arms.side(side) else {
            c.text(x0 + 8, TOP + 8, "NO ARM", [200, 120, 120]);
            continue;
        };
        let wrist = mesh.bind.get(arm.wrist as usize).copied().unwrap_or(IDENTITY);
        let to_arm = arm.to_arm(&wrist);
        let tris: Vec<[[f32; 3]; 3]> = arm
            .triangles
            .iter()
            .filter_map(|&t| mesh.triangles.get(t as usize))
            .map(|t| t.map(|v| transform_point(&to_arm, mesh.positions[v as usize])))
            .collect();
        let view = |p: [f32; 3]| [p[2], p[1], -p[0]];
        let mut used: Vec<[f32; 3]> = tris.iter().flat_map(|t| t.iter().map(|&p| view(p))).collect();
        used.push([0.0; 3]);
        let fit = Fit::new(&used, P, TOP, x0);
        let col = colour(side);
        for t in &tris {
            let [a, b, d] = t.map(|p| fit.px(view(p)));
            let n = cross(sub(t[1], t[0]), sub(t[2], t[0]));
            let lit = normalize(n).map_or(0.6, |n| 0.45 + 0.55 * n[0].abs());
            c.tri(a, b, d, col.map(|x| (x as f32 * lit) as u8));
        }
        let o = fit.px([0.0; 3]);
        c.line(o, fit.px([fit.span * 0.3, 0.0, 0.0]), [240, 240, 240]);
        c.line(o, fit.px([0.0, fit.span * 0.3, 0.0]), [120, 230, 120]);
        c.text(x0 + 8, TOP + 8, if side == 0 { "RIGHT  Z>" } else { "LEFT  Z>" }, [235, 235, 235]);
        c.text(x0 + 8, TOP + 20, &format!("{} TRIS", arm.triangles.len()), [200, 200, 200]);
    }
    (w as u32, h as u32, c.px)
}

const IDENTITY: [f32; 16] = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];

fn shade(col: [u8; 3], _mesh: &Mesh, t: &[u32; 3], pts: &[[f32; 3]], axis: usize) -> [u8; 3] {
    let [a, b, c] = t.map(|v| pts[v as usize]);
    let n = cross(sub(b, a), sub(c, a));
    let lit = normalize(n).map_or(0.6, |n| 0.4 + 0.6 * n[if axis == 0 { 2 } else { 0 }].abs());
    col.map(|x| (x as f32 * lit) as u8)
}

struct Fit {
    lo: [f32; 2],
    hi_y: f32,
    s: f32,
    off: [f32; 2],
    span: f32,
}

impl Fit {
    fn new(pts: &[[f32; 3]], p: usize, top: usize, x0: usize) -> Fit {
        let (mut lo, mut hi) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
        for q in pts {
            for i in 0..2 {
                lo[i] = lo[i].min(q[i]);
                hi[i] = hi[i].max(q[i]);
            }
        }
        if !lo[0].is_finite() {
            lo = [-1.0; 2];
            hi = [1.0; 2];
        }
        let span = (hi[0] - lo[0]).max(hi[1] - lo[1]).max(1e-3);
        let s = (p as f32 - 24.0) / span;
        let off = [
            x0 as f32 + (p as f32 - (hi[0] - lo[0]) * s) * 0.5,
            top as f32 + (p as f32 - (hi[1] - lo[1]) * s) * 0.5,
        ];
        Fit { lo, hi_y: hi[1], s, off, span }
    }
    // (x right, y up, depth) to pixels (y down from the fitted box's top).
    fn px(&self, p: [f32; 3]) -> [f32; 3] {
        let x = self.off[0] + (p[0] - self.lo[0]) * self.s;
        let y = self.off[1] + (self.hi_y - p[1]) * self.s;
        [x, y, p[2]]
    }
}

struct Canvas {
    w: usize,
    h: usize,
    px: Vec<u8>,
    depth: Vec<f32>,
}

impl Canvas {
    fn new(w: usize, h: usize) -> Canvas {
        let mut px = vec![0u8; w * h * 4];
        for p in px.chunks_exact_mut(4) {
            p.copy_from_slice(&[24, 26, 32, 255]);
        }
        Canvas { w, h, px, depth: vec![f32::INFINITY; w * h] }
    }

    fn set(&mut self, x: i32, y: i32, c: [u8; 3]) {
        if x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h {
            return;
        }
        let i = (y as usize * self.w + x as usize) * 4;
        self.px[i..i + 3].copy_from_slice(&c);
    }

    // A flat triangle in pixels (x, y, depth: nearer is smaller).
    fn tri(&mut self, a: [f32; 3], b: [f32; 3], c: [f32; 3], col: [u8; 3]) {
        let area = (b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1]);
        if area.abs() < 1e-6 {
            return;
        }
        let x0 = a[0].min(b[0]).min(c[0]).floor().max(0.0) as i32;
        let x1 = a[0].max(b[0]).max(c[0]).ceil().min(self.w as f32 - 1.0) as i32;
        let y0 = a[1].min(b[1]).min(c[1]).floor().max(0.0) as i32;
        let y1 = a[1].max(b[1]).max(c[1]).ceil().min(self.h as f32 - 1.0) as i32;
        for y in y0..=y1 {
            for x in x0..=x1 {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let w0 = ((b[0] - px) * (c[1] - py) - (c[0] - px) * (b[1] - py)) / area;
                let w1 = ((c[0] - px) * (a[1] - py) - (a[0] - px) * (c[1] - py)) / area;
                let w2 = 1.0 - w0 - w1;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let z = w0 * a[2] + w1 * b[2] + w2 * c[2];
                let i = y as usize * self.w + x as usize;
                if z < self.depth[i] {
                    self.depth[i] = z;
                    self.set(x, y, col);
                }
            }
        }
    }

    fn line(&mut self, a: [f32; 3], b: [f32; 3], col: [u8; 3]) {
        let n = ((b[0] - a[0]).abs().max((b[1] - a[1]).abs()) as i32).max(1);
        for i in 0..=n {
            let t = i as f32 / n as f32;
            self.set((a[0] + (b[0] - a[0]) * t) as i32, (a[1] + (b[1] - a[1]) * t) as i32, col);
        }
    }

    // 3×5 capitals, digits and a few marks, two pixels a dot.
    fn text(&mut self, x: usize, y: usize, s: &str, col: [u8; 3]) {
        let mut cx = x as i32;
        for ch in s.chars() {
            let rows = glyph(ch.to_ascii_uppercase());
            for (ry, row) in rows.iter().enumerate() {
                for rx in 0..3 {
                    if row & (4 >> rx) != 0 {
                        for d in 0..4 {
                            self.set(cx + rx * 2 + d % 2, y as i32 + ry as i32 * 2 + d / 2, col);
                        }
                    }
                }
            }
            cx += 8;
        }
    }
}

fn glyph(c: char) -> [u8; 5] {
    match c {
        'A' => [2, 5, 7, 5, 5],
        'B' => [6, 5, 6, 5, 6],
        'C' => [3, 4, 4, 4, 3],
        'D' => [6, 5, 5, 5, 6],
        'E' => [7, 4, 6, 4, 7],
        'F' => [7, 4, 6, 4, 4],
        'G' => [3, 4, 5, 5, 3],
        'H' => [5, 5, 7, 5, 5],
        'I' => [7, 2, 2, 2, 7],
        'J' => [1, 1, 1, 5, 2],
        'K' => [5, 5, 6, 5, 5],
        'L' => [4, 4, 4, 4, 7],
        'M' => [5, 7, 7, 5, 5],
        'N' => [6, 5, 5, 5, 5],
        'O' => [2, 5, 5, 5, 2],
        'P' => [6, 5, 6, 4, 4],
        'Q' => [2, 5, 5, 6, 3],
        'R' => [6, 5, 6, 5, 5],
        'S' => [3, 4, 2, 1, 6],
        'T' => [7, 2, 2, 2, 2],
        'U' => [5, 5, 5, 5, 7],
        'V' => [5, 5, 5, 5, 2],
        'W' => [5, 5, 7, 7, 5],
        'X' => [5, 5, 2, 5, 5],
        'Y' => [5, 5, 2, 2, 2],
        'Z' => [7, 1, 2, 4, 7],
        '0' => [7, 5, 5, 5, 7],
        '1' => [2, 6, 2, 2, 7],
        '2' => [6, 1, 2, 4, 7],
        '3' => [6, 1, 2, 1, 6],
        '4' => [5, 5, 7, 1, 1],
        '5' => [7, 4, 6, 1, 6],
        '6' => [3, 4, 7, 5, 7],
        '7' => [7, 1, 2, 2, 2],
        '8' => [7, 5, 7, 5, 7],
        '9' => [7, 5, 7, 1, 6],
        '>' => [4, 2, 1, 2, 4],
        '-' => [0, 0, 7, 0, 0],
        '_' => [0, 0, 0, 0, 7],
        '/' => [1, 1, 2, 4, 4],
        '.' => [0, 0, 0, 0, 2],
        ':' => [0, 2, 0, 2, 0],
        '(' => [1, 2, 2, 2, 1],
        ')' => [4, 2, 2, 2, 4],
        _ => [0; 5],
    }
}

/// An RGBA8 picture (rows top first) as a PNG (stored zlib blocks: big, but no dependencies).
pub fn png(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
    let mut raw = Vec::with_capacity(rgba.len() + h as usize);
    for row in rgba.chunks_exact(w as usize * 4) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    let mut z = vec![0x78, 0x01];
    let blocks: Vec<_> = raw.chunks(65535).collect();
    for (i, b) in blocks.iter().enumerate() {
        z.push(u8::from(i + 1 == blocks.len()));
        z.extend_from_slice(&(b.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(b.len() as u16)).to_le_bytes());
        z.extend_from_slice(b);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &x in &raw {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    z.extend_from_slice(&(b << 16 | a).to_be_bytes());
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut chunk = |tag: &[u8], bytes: &[u8]| {
        out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        let mut data = tag.to_vec();
        data.extend_from_slice(bytes);
        out.extend_from_slice(&data);
        out.extend_from_slice(&crc32(&data).to_be_bytes());
    };
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(b"IHDR", &ihdr);
    chunk(b"IDAT", &z);
    chunk(b"IEND", &[]);
    out
}

fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
    }
    !c
}

#[cfg(test)]
mod tests {
    use super::*;

    fn translate(x: f32, y: f32, z: f32) -> [f32; 16] {
        let mut m = IDENTITY;
        m[12] = x;
        m[13] = y;
        m[14] = z;
        m
    }

    // A T-posed stick figure: a body, and a right arm along +x (shoulder, elbow at 2, wrist at 4).
    fn figure() -> Mesh {
        let parents = vec![None, Some(0), Some(1), Some(2)];
        let bind = vec![translate(0.0, 0.0, 0.0), translate(1.0, 0.0, 0.0), translate(2.0, 0.0, 0.0), translate(4.0, 0.0, 0.0)];
        let mut m = Mesh { parents, bind, ..Mesh::default() };
        // One quad (two triangles) per segment, each skinned to that segment's joint.
        for (j, (x0, x1)) in [(0u16, (-1.0, 1.0)), (1, (1.0, 2.0)), (2, (2.0, 4.0)), (3, (4.0, 5.0))] {
            let base = m.positions.len() as u32;
            for p in [[x0, -0.3, 0.0], [x1, -0.3, 0.0], [x1, 0.3, 0.0], [x0, 0.3, 0.0]] {
                m.positions.push(p);
                m.weights.push(vec![(j, 1.0)]);
            }
            m.triangles.push([base, base + 1, base + 2]);
            m.triangles.push([base, base + 2, base + 3]);
        }
        m
    }

    #[test]
    fn keeps_the_forearm_and_hand_and_finds_arm_space() {
        let m = figure();
        let side = Side::from_wrist(&m, 3).unwrap();
        assert_eq!(side, Side { elbow: 2, wrist: 3 });
        let arms = cut("figure", &m, [Some(side), None]);
        let arm = arms.side(0).unwrap();
        // The forearm's two and the hand's two; not the body's or the upper arm's.
        assert_eq!(arm.triangles, vec![4, 5, 6, 7]);
        assert!(arms.side(1).is_none());
        // Arm space: the hand's middle (4.5, 0, 0) at the origin, +z along +x.
        let to_arm = arm.to_arm(&m.bind[3]);
        let hand = transform_point(&to_arm, [4.5, 0.0, 0.0]);
        assert!(hand.iter().all(|v| v.abs() < 1e-4), "{hand:?}");
        let elbow = transform_point(&to_arm, [2.0, 0.0, 0.0]);
        assert!((elbow[2] + 2.5).abs() < 1e-4 && elbow[0].abs() < 1e-4, "{elbow:?}");
        let up = transform_point(&to_arm, [4.5, 1.0, 0.0]);
        assert!((up[1] - 1.0).abs() < 1e-4, "{up:?}");
        // Posed: the whole arm lifted and turned keeps the same arm-space shape.
        let turn = [0.0, 1.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];
        let lift = mul(&translate(0.0, 3.0, 0.0), &turn);
        let wrist_now = mul(&lift, &m.bind[3]);
        let hand_now = transform_point(&lift, [4.5, 0.0, 0.0]);
        let back = transform_point(&arm.to_arm(&wrist_now), hand_now);
        assert!(back.iter().all(|v| v.abs() < 1e-4), "{back:?}");
    }

    #[test]
    fn shared_seam_vertices_snap_onto_the_arm() {
        // Two triangles sharing an edge: (0 1 2) on the arm, (1 3 2) off it.
        let tris = [[0, 1, 2], [1, 3, 2], [4, 5, 6]];
        let arm = Arm { side: 0, elbow: 0, wrist: 0, frame: IDENTITY, triangles: vec![0] };
        let map = arm.vertex_map(&tris, 7);
        assert_eq!(&map[..3], &[0, 1, 2]);
        assert!(map[3] == 1 || map[3] == 2, "{map:?}");
        assert!(map[4..].iter().all(|&m| m == COLLAPSED));
    }

    #[test]
    fn a_one_joint_limb_is_its_own_subtree() {
        let m = figure();
        let arms = cut("figure", &m, [Some(Side::limb(3)), None]);
        let arm = arms.side(0).unwrap();
        assert_eq!(arm.triangles, vec![6, 7]);
        // The hand's middle at the origin, +z out along +x from its joint.
        let to_arm = arm.to_arm(&m.bind[3]);
        let tip = transform_point(&to_arm, [5.0, 0.0, 0.0]);
        assert!(tip[2] > 0.4 && tip[0].abs() < 1e-4, "{tip:?}");
    }

    #[test]
    fn the_text_round_trips_and_rejects_other_versions() {
        let m = figure();
        let arms = cut("figure", &m, [Side::from_wrist(&m, 3), None]);
        let text = arms.to_text();
        assert_eq!(Arms::parse(&text), Some(arms.clone()));
        assert!(arms.fits(16, 8) && !arms.fits(16, 9));
        assert!(Arms::parse(&text.replace("vr-arms 1", "vr-arms 0")).is_none());
        let (w, h, px) = capture(&m, &arms, "figure");
        assert_eq!(px.len(), (w * h * 4) as usize);
        assert!(png(w, h, &px).starts_with(b"\x89PNG"));
    }
}
