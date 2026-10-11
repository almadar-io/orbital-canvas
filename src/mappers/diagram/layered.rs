//! Layered (Sugiyama-style) layout for state-machine diagrams. The web draws these with ELK
//! (`avl-elk-layout.ts`: layered, orthogonal routing, model-order cycle breaking, initial state
//! pinned to the first layer, inline edge labels); ELK has no Rust build here, so this lays the
//! same inputs out deterministically: DFS back-edge cycle breaking, longest-path layers, dummy
//! chains for long edges, barycenter ordering, isotonic y placement, orthogonal routing with a
//! label slot per edge. Coordinates differ from ELK's; the structure (left-to-right flow, labels
//! on their edges, back edges looping around the top) is the same.

use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Debug, Clone)]
pub struct NodeIn {
    pub w: f32,
    pub h: f32,
    pub initial: bool,
}

#[derive(Debug, Clone)]
pub struct EdgeIn {
    pub from: usize,
    pub to: usize,
    pub label_w: f32,
    pub label_h: f32,
}

#[derive(Debug, Clone, Copy)]
pub struct Metrics {
    pub node_spacing: f32,
    pub layer_spacing: f32,
    pub rtl: bool,
}

#[derive(Debug, Clone)]
pub struct EdgeOut {
    pub points: Vec<(f32, f32)>,
    pub label: Rect,
    pub is_self: bool,
    pub is_backward: bool,
}

#[derive(Debug, Clone)]
pub struct Layout {
    pub nodes: Vec<Rect>,
    pub edges: Vec<EdgeOut>,
    pub width: f32,
    pub height: f32,
}

const LABEL_PAD: f32 = 14.0;
const TRACK: f32 = 6.0;
const LABEL_GAP: f32 = 6.0;
const DUMMY_SPACING: f32 = 12.0;
const LOOP_RISE: f32 = 16.0;
const TRACK_RISE: f32 = 20.0;

/// States in breadth-first order from the initial state(s); the rest keep declaration order.
fn flow_order(nodes: &[NodeIn], edges: &[EdgeIn]) -> Vec<usize> {
    let mut seen = vec![false; nodes.len()];
    let mut order = Vec::new();
    let mut queue: std::collections::VecDeque<usize> = nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| n.initial)
        .map(|(i, _)| i)
        .collect();
    while let Some(n) = queue.pop_front() {
        if seen[n] {
            continue;
        }
        seen[n] = true;
        order.push(n);
        for e in edges.iter().filter(|e| e.from == n && !seen[e.to]) {
            queue.push_back(e.to);
        }
    }
    order.extend(
        seen.iter()
            .enumerate()
            .filter(|(_, s)| !**s)
            .map(|(i, _)| i),
    );
    order
}

/// Edge indices that close a cycle (DFS in flow order) or enter an initial node.
fn back_edges(nodes: &[NodeIn], edges: &[EdgeIn], order: &[usize]) -> HashSet<usize> {
    fn visit(n: usize, edges: &[EdgeIn], state: &mut [u8], back: &mut HashSet<usize>) {
        state[n] = 1;
        for (i, e) in edges
            .iter()
            .enumerate()
            .filter(|(_, e)| e.from == n && e.to != n)
        {
            match state[e.to] {
                1 => {
                    back.insert(i);
                }
                0 => visit(e.to, edges, state, back),
                _ => {}
            }
        }
        state[n] = 2;
    }
    let mut state = vec![0u8; nodes.len()];
    let mut back = HashSet::new();
    for &n in order {
        if state[n] == 0 {
            visit(n, edges, &mut state, &mut back);
        }
    }
    for (i, e) in edges.iter().enumerate() {
        if e.from != e.to && nodes[e.to].initial && !nodes[e.from].initial {
            back.insert(i);
        }
    }
    back
}

/// Least-squares positions `y` with `y[i+1] - y[i] >= sep[i]` closest to `want` (pool adjacent violators).
fn isotonic(want: &[f32], sep: &[f32]) -> Vec<f32> {
    let n = want.len();
    if n == 0 {
        return Vec::new();
    }
    let mut cum = vec![0.0f32; n];
    for i in 1..n {
        cum[i] = cum[i - 1] + sep[i - 1];
    }
    let z: Vec<f32> = (0..n).map(|i| want[i] - cum[i]).collect();
    let mut blocks: Vec<(f32, usize)> = Vec::new();
    for v in z {
        blocks.push((v, 1));
        while blocks.len() > 1 {
            let (b, bn) = blocks[blocks.len() - 1];
            let (a, an) = blocks[blocks.len() - 2];
            if a <= b {
                break;
            }
            blocks.truncate(blocks.len() - 2);
            blocks.push(((a * an as f32 + b * bn as f32) / (an + bn) as f32, an + bn));
        }
    }
    let mut out = Vec::with_capacity(n);
    for (v, count) in blocks {
        for _ in 0..count {
            out.push(v);
        }
    }
    (0..n).map(|i| out[i] + cum[i]).collect()
}

#[derive(Clone)]
struct Item {
    real: Option<usize>,
    w: f32,
    h: f32,
    layer: usize,
    x: f32,
    cy: f32,
}

struct Chain {
    edge: usize,
    items: Vec<usize>,
}

pub fn layout(nodes: &[NodeIn], edges: &[EdgeIn], m: Metrics) -> Layout {
    if nodes.is_empty() {
        return Layout {
            nodes: Vec::new(),
            edges: Vec::new(),
            width: 0.0,
            height: 0.0,
        };
    }
    let order = flow_order(nodes, edges);
    let rank: Vec<usize> = {
        let mut r = vec![0; nodes.len()];
        for (i, &n) in order.iter().enumerate() {
            r[n] = i;
        }
        r
    };
    let back = back_edges(nodes, edges, &order);
    let forward: Vec<usize> = (0..edges.len())
        .filter(|i| edges[*i].from != edges[*i].to && !back.contains(i))
        .collect();

    let mut layer = vec![0usize; nodes.len()];
    loop {
        let mut changed = false;
        for &i in &forward {
            let e = &edges[i];
            if layer[e.to] < layer[e.from] + 1 {
                layer[e.to] = layer[e.from] + 1;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    let mut items: Vec<Item> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| Item {
            real: Some(i),
            w: n.w,
            h: n.h,
            layer: layer[i],
            x: 0.0,
            cy: 0.0,
        })
        .collect();
    let mut chains: Vec<Chain> = Vec::new();
    for &i in &forward {
        let e = &edges[i];
        let mut chain = vec![e.from];
        for l in layer[e.from] + 1..layer[e.to] {
            items.push(Item {
                real: None,
                w: 0.0,
                h: 0.0,
                layer: l,
                x: 0.0,
                cy: 0.0,
            });
            chain.push(items.len() - 1);
        }
        chain.push(e.to);
        chains.push(Chain {
            edge: i,
            items: chain,
        });
    }
    let layer_count = items.iter().map(|it| it.layer).max().unwrap_or(0) + 1;

    let mut cols: Vec<Vec<usize>> = vec![Vec::new(); layer_count];
    let mut seeded: Vec<usize> = (0..items.len()).collect();
    seeded.sort_by_key(|&i| match items[i].real {
        Some(n) => (rank[n], 0),
        None => (usize::MAX, i),
    });
    for i in seeded {
        cols[items[i].layer].push(i);
    }
    let mut preds: Vec<Vec<usize>> = vec![Vec::new(); items.len()];
    let mut succs: Vec<Vec<usize>> = vec![Vec::new(); items.len()];
    for c in &chains {
        for w in c.items.windows(2) {
            succs[w[0]].push(w[1]);
            preds[w[1]].push(w[0]);
        }
    }
    let mut pos = vec![0usize; items.len()];
    let reindex = |cols: &Vec<Vec<usize>>, pos: &mut Vec<usize>| {
        for col in cols {
            for (p, &i) in col.iter().enumerate() {
                pos[i] = p;
            }
        }
    };
    reindex(&cols, &mut pos);
    for sweep in 0..8 {
        let down = sweep % 2 == 0;
        let range: Vec<usize> = if down {
            (1..layer_count).collect()
        } else {
            (0..layer_count.saturating_sub(1)).rev().collect()
        };
        for l in range {
            let neigh = if down { &preds } else { &succs };
            let mut keyed: Vec<(usize, f32)> = cols[l]
                .iter()
                .map(|&i| {
                    let ns = &neigh[i];
                    let b = if ns.is_empty() {
                        pos[i] as f32
                    } else {
                        ns.iter().map(|&n| pos[n] as f32).sum::<f32>() / ns.len() as f32
                    };
                    (i, b)
                })
                .collect();
            keyed.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
            cols[l] = keyed.into_iter().map(|(i, _)| i).collect();
            reindex(&cols, &mut pos);
        }
    }

    let first_gap_labels = |l: usize| -> f32 {
        chains
            .iter()
            .filter(|c| items[c.items[0]].layer == l)
            .map(|c| edges[c.edge].label_w)
            .fold(0.0, f32::max)
    };
    let crossing = |l: usize| {
        chains
            .iter()
            .filter(|c| {
                items[c.items[0]].layer <= l && items[*c.items.last().unwrap_or(&0)].layer > l
            })
            .count()
    };
    let mut gaps = Vec::new();
    for l in 0..layer_count.saturating_sub(1) {
        let need = first_gap_labels(l) + 2.0 * LABEL_PAD + TRACK * crossing(l) as f32;
        gaps.push(m.layer_spacing.max(need));
    }
    let widths: Vec<f32> = cols
        .iter()
        .map(|c| c.iter().map(|&i| items[i].w).fold(0.0, f32::max))
        .collect();
    let mut col_x = vec![0.0f32; layer_count];
    for l in 1..layer_count {
        col_x[l] = col_x[l - 1] + widths[l - 1] + gaps[l - 1];
    }
    for (l, col) in cols.iter().enumerate() {
        for &i in col {
            items[i].x = col_x[l] + (widths[l] - items[i].w) / 2.0;
        }
    }

    for col in &cols {
        let mut y = 0.0;
        for &i in col {
            items[i].cy = y + items[i].h / 2.0;
            y += items[i].h + m.node_spacing;
        }
    }
    let sep = |a: &Item, b: &Item| {
        let gap = match (a.real.is_some(), b.real.is_some()) {
            (true, true) => m.node_spacing,
            (false, false) => DUMMY_SPACING,
            _ => m.node_spacing / 2.0,
        };
        (a.h + b.h) / 2.0 + gap
    };
    for sweep in 0..10 {
        let down = sweep % 2 == 0;
        let range: Vec<usize> = if down {
            (1..layer_count).collect()
        } else {
            (0..layer_count.saturating_sub(1)).rev().collect()
        };
        for l in range {
            let neigh = if down { &preds } else { &succs };
            let want: Vec<f32> = cols[l]
                .iter()
                .map(|&i| {
                    let ns = &neigh[i];
                    if ns.is_empty() {
                        items[i].cy
                    } else {
                        ns.iter().map(|&n| items[n].cy).sum::<f32>() / ns.len() as f32
                    }
                })
                .collect();
            let seps: Vec<f32> = cols[l]
                .windows(2)
                .map(|w| sep(&items[w[0]], &items[w[1]]))
                .collect();
            let placed = isotonic(&want, &seps);
            for (k, &i) in cols[l].iter().enumerate() {
                items[i].cy = placed[k];
            }
        }
    }

    // ports: outgoing/incoming edge ends spread along a node's east/west side, ordered by the other end's y.
    let n_items = items.len();
    let mut out_ports: Vec<Vec<(usize, usize)>> = vec![Vec::new(); n_items];
    let mut in_ports: Vec<Vec<(usize, usize)>> = vec![Vec::new(); n_items];
    for (ci, c) in chains.iter().enumerate() {
        out_ports[c.items[0]].push((ci, 0));
        in_ports[*c.items.last().unwrap_or(&0)].push((ci, c.items.len() - 2));
    }
    let port_y = |item: usize,
                  which: &Vec<Vec<(usize, usize)>>,
                  key: &dyn Fn(usize, usize) -> f32,
                  ci: usize|
     -> f32 {
        let mut ports = which[item].clone();
        ports.sort_by(|a, b| {
            key(a.0, a.1)
                .partial_cmp(&key(b.0, b.1))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let n = ports.len();
        let k = ports.iter().position(|p| p.0 == ci).unwrap_or(0);
        let it = &items[item];
        if it.real.is_none() || n <= 1 {
            it.cy
        } else {
            it.cy - it.h / 2.0 + it.h * (k as f32 + 1.0) / (n as f32 + 1.0)
        }
    };
    let next_y = |ci: usize, seg: usize| items[chains[ci].items[seg + 1]].cy;
    let prev_y = |ci: usize, seg: usize| items[chains[ci].items[seg]].cy;
    let mut seg_ends: Vec<Vec<(f32, f32)>> = Vec::new();
    for (ci, c) in chains.iter().enumerate() {
        let mut ends = Vec::new();
        for seg in 0..c.items.len() - 1 {
            let a = c.items[seg];
            let b = c.items[seg + 1];
            let ay = if seg == 0 {
                port_y(a, &out_ports, &|c2, s| next_y(c2, s), ci)
            } else {
                items[a].cy
            };
            let by = if seg == c.items.len() - 2 {
                port_y(b, &in_ports, &|c2, s| prev_y(c2, s), ci)
            } else {
                items[b].cy
            };
            ends.push((ay, by));
        }
        seg_ends.push(ends);
    }

    // label slots in each edge's first gap, stacked by desired y.
    let mut label_y = vec![0.0f32; chains.len()];
    let mut label_cx = vec![0.0f32; chains.len()];
    for l in 0..layer_count.saturating_sub(1) {
        let mut here: Vec<usize> = (0..chains.len())
            .filter(|&ci| items[chains[ci].items[0]].layer == l)
            .collect();
        if here.is_empty() {
            continue;
        }
        let want_of = |ci: usize| (seg_ends[ci][0].0 + seg_ends[ci][0].1) / 2.0;
        here.sort_by(|a, b| {
            want_of(*a)
                .partial_cmp(&want_of(*b))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let want: Vec<f32> = here.iter().map(|&ci| want_of(ci)).collect();
        let seps: Vec<f32> = here
            .windows(2)
            .map(|w| {
                (edges[chains[w[0]].edge].label_h + edges[chains[w[1]].edge].label_h) / 2.0
                    + LABEL_GAP
            })
            .collect();
        let placed = isotonic(&want, &seps);
        let gx = col_x[l] + widths[l] + gaps[l] / 2.0;
        for (k, &ci) in here.iter().enumerate() {
            label_y[ci] = placed[k];
            label_cx[ci] = gx;
        }
    }

    let mut edge_out: Vec<Option<EdgeOut>> = vec![None; edges.len()];
    for (ci, c) in chains.iter().enumerate() {
        let e = &edges[c.edge];
        let mut pts: Vec<(f32, f32)> = Vec::new();
        let k_by_seg = |seg: usize| -> (usize, usize) {
            let l = items[c.items[seg]].layer;
            let mut crossing_here: Vec<usize> = (0..chains.len())
                .filter(|&o| {
                    let first = items[chains[o].items[0]].layer;
                    first <= l && first + chains[o].items.len() - 1 > l
                })
                .collect();
            crossing_here.sort_by(|a, b| {
                let key = |o: usize| {
                    let s = l - items[chains[o].items[0]].layer;
                    (seg_ends[o][s].0 + seg_ends[o][s].1) / 2.0
                };
                key(*a)
                    .partial_cmp(&key(*b))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            (
                crossing_here.iter().position(|&o| o == ci).unwrap_or(0),
                crossing_here.len(),
            )
        };
        for seg in 0..c.items.len() - 1 {
            let a = &items[c.items[seg]];
            let b = &items[c.items[seg + 1]];
            let (ay, by) = seg_ends[ci][seg];
            let (ax, bx) = (a.x + a.w, b.x);
            let (k, count) = k_by_seg(seg);
            if seg == 0 {
                let x1 = ax + LABEL_PAD / 2.0 + TRACK * k as f32;
                let x2 = bx - LABEL_PAD / 2.0 - TRACK * (count - 1 - k) as f32;
                pts.extend([
                    (ax, ay),
                    (x1, ay),
                    (x1, label_y[ci]),
                    (x2, label_y[ci]),
                    (x2, by),
                    (bx, by),
                ]);
            } else {
                let xm = (ax + bx) / 2.0 + TRACK * (k as f32 - (count as f32 - 1.0) / 2.0);
                pts.extend([(ax, ay), (xm, ay), (xm, by), (bx, by)]);
            }
        }
        edge_out[c.edge] = Some(EdgeOut {
            points: tidy(pts),
            label: Rect {
                x: label_cx[ci] - e.label_w / 2.0,
                y: label_y[ci] - e.label_h / 2.0,
                w: e.label_w,
                h: e.label_h,
            },
            is_self: false,
            is_backward: false,
        });
    }

    let mut node_rects: Vec<Rect> = (0..nodes.len())
        .map(|i| Rect {
            x: items[i].x,
            y: items[i].cy - items[i].h / 2.0,
            w: items[i].w,
            h: items[i].h,
        })
        .collect();
    let mut min_top = node_rects.iter().map(|r| r.y).fold(f32::INFINITY, f32::min);
    let mut loops_at = vec![0usize; nodes.len()];
    for (i, e) in edges.iter().enumerate() {
        if e.from == e.to {
            let r = node_rects[e.from];
            let j = loops_at[e.from];
            loops_at[e.from] += 1;
            let rise = LOOP_RISE + (e.label_h + LABEL_GAP) * (j as f32 + 1.0);
            let (lx0, lx1) = (r.x + r.w * 0.7, r.x + r.w * 0.3);
            let top = r.y;
            edge_out[i] = Some(EdgeOut {
                points: vec![
                    (lx0, top),
                    (lx0, top - LOOP_RISE),
                    (lx1, top - LOOP_RISE),
                    (lx1, top),
                ],
                label: Rect {
                    x: r.x + r.w / 2.0 - e.label_w / 2.0,
                    y: top - rise - e.label_h / 2.0 + LOOP_RISE - LABEL_GAP,
                    w: e.label_w,
                    h: e.label_h,
                },
                is_self: true,
                is_backward: false,
            });
            min_top = min_top.min(top - rise - e.label_h / 2.0);
        }
    }
    let mut track_y = min_top - TRACK_RISE;
    let mut next_back = 0usize;
    for (i, e) in edges.iter().enumerate() {
        if !back.contains(&i) {
            continue;
        }
        let (s, t) = (node_rects[e.from], node_rects[e.to]);
        let j = next_back as f32;
        next_back += 1;
        let (sx, tx) = (s.x + s.w * 0.5 + TRACK * j, t.x + t.w * 0.5 - TRACK * j);
        edge_out[i] = Some(EdgeOut {
            points: vec![(sx, s.y), (sx, track_y), (tx, track_y), (tx, t.y)],
            label: Rect {
                x: (sx + tx) / 2.0 - e.label_w / 2.0,
                y: track_y - e.label_h / 2.0,
                w: e.label_w,
                h: e.label_h,
            },
            is_self: false,
            is_backward: true,
        });
        track_y -= e.label_h + 8.0;
    }

    let mut out: Vec<EdgeOut> = edge_out
        .into_iter()
        .map(|e| {
            e.unwrap_or(EdgeOut {
                points: Vec::new(),
                label: Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 0.0,
                    h: 0.0,
                },
                is_self: false,
                is_backward: false,
            })
        })
        .collect();
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    let mut grow = |x: f32, y: f32| {
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x);
        max_y = max_y.max(y);
    };
    for r in &node_rects {
        grow(r.x, r.y);
        grow(r.x + r.w, r.y + r.h);
    }
    for e in &out {
        for p in &e.points {
            grow(p.0, p.1);
        }
        if e.label.w > 0.0 {
            grow(e.label.x, e.label.y);
            grow(e.label.x + e.label.w, e.label.y + e.label.h);
        }
    }
    let (width, height) = (max_x - min_x, max_y - min_y);
    for r in &mut node_rects {
        r.x -= min_x;
        r.y -= min_y;
    }
    for e in &mut out {
        for p in &mut e.points {
            p.0 -= min_x;
            p.1 -= min_y;
        }
        e.label.x -= min_x;
        e.label.y -= min_y;
    }
    if m.rtl {
        for r in &mut node_rects {
            r.x = width - (r.x + r.w);
        }
        for e in &mut out {
            for p in &mut e.points {
                p.0 = width - p.0;
            }
            e.label.x = width - (e.label.x + e.label.w);
        }
    }
    Layout {
        nodes: node_rects,
        edges: out,
        width,
        height,
    }
}

/// Drop repeated and collinear points so an orthogonal route keeps only its bends.
fn tidy(points: Vec<(f32, f32)>) -> Vec<(f32, f32)> {
    let mut out: Vec<(f32, f32)> = Vec::new();
    for p in points {
        if out
            .last()
            .is_some_and(|q| (q.0 - p.0).abs() < 0.01 && (q.1 - p.1).abs() < 0.01)
        {
            continue;
        }
        while out.len() >= 2 {
            let (a, b) = (out[out.len() - 2], out[out.len() - 1]);
            let same_x = (a.0 - b.0).abs() < 0.01 && (b.0 - p.0).abs() < 0.01;
            let same_y = (a.1 - b.1).abs() < 0.01 && (b.1 - p.1).abs() < 0.01;
            if same_x || same_y {
                out.pop();
            } else {
                break;
            }
        }
        out.push(p);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const M: Metrics = Metrics {
        node_spacing: 40.0,
        layer_spacing: 88.0,
        rtl: false,
    };

    fn node(w: f32, initial: bool) -> NodeIn {
        NodeIn {
            w,
            h: 44.0,
            initial,
        }
    }

    fn edge(from: usize, to: usize) -> EdgeIn {
        EdgeIn {
            from,
            to,
            label_w: 60.0,
            label_h: 24.0,
        }
    }

    #[test]
    fn a_chain_flows_left_to_right_on_one_row_with_straight_wires() {
        let l = layout(
            &[node(100.0, true), node(100.0, false), node(100.0, false)],
            &[edge(0, 1), edge(1, 2)],
            M,
        );
        assert!(l.nodes[0].x < l.nodes[1].x && l.nodes[1].x < l.nodes[2].x);
        assert_eq!(l.nodes[0].y, l.nodes[1].y);
        for e in &l.edges {
            assert_eq!(
                e.points.len(),
                2,
                "a straight row needs no bends: {:?}",
                e.points
            );
            assert!(!e.is_backward && !e.is_self);
        }
        assert!(
            l.nodes[1].x - (l.nodes[0].x + 100.0) >= 88.0,
            "layer spacing honored"
        );
    }

    #[test]
    fn a_back_edge_loops_over_the_top_and_marks_itself_backward() {
        let l = layout(
            &[node(100.0, true), node(100.0, false)],
            &[edge(0, 1), edge(1, 0)],
            M,
        );
        assert!(l.edges[1].is_backward && !l.edges[0].is_backward);
        let top = l.nodes.iter().map(|r| r.y).fold(f32::INFINITY, f32::min);
        assert!(
            l.edges[1].points.iter().any(|p| p.1 < top),
            "the track runs above every node"
        );
        assert_eq!(
            l.nodes[0].x,
            l.nodes[0].x.min(l.nodes[1].x),
            "the initial state stays first"
        );
    }

    #[test]
    fn a_self_loop_rises_above_its_node_and_a_fork_stacks_its_targets() {
        let l = layout(
            &[node(100.0, true), node(100.0, false), node(100.0, false)],
            &[edge(0, 0), edge(0, 1), edge(0, 2)],
            M,
        );
        assert!(l.edges[0].is_self);
        assert!(l.edges[0].points.iter().all(|p| p.1 <= l.nodes[0].y + 0.01));
        assert!(
            l.nodes[1].y != l.nodes[2].y,
            "two targets of one layer take separate rows"
        );
        let gap = (l.nodes[1].y - l.nodes[2].y).abs();
        assert!(
            gap >= 44.0 + 40.0 - 0.01,
            "node spacing keeps rows apart: {gap}"
        );
    }

    #[test]
    fn labels_in_one_gap_never_overlap_and_every_label_sits_between_its_nodes() {
        let nodes = [
            node(100.0, true),
            node(100.0, false),
            node(100.0, false),
            node(100.0, false),
        ];
        let edges = [edge(0, 1), edge(0, 2), edge(0, 3), edge(1, 3)];
        let l = layout(&nodes, &edges, M);
        for a in 0..3 {
            for b in a + 1..3 {
                let (p, q) = (l.edges[a].label, l.edges[b].label);
                let overlap =
                    p.x < q.x + q.w && q.x < p.x + p.w && p.y < q.y + q.h && q.y < p.y + p.h;
                assert!(!overlap, "labels {a} and {b} overlap: {p:?} {q:?}");
            }
        }
        let first = &l.edges[0];
        assert!(
            first.label.x >= l.nodes[0].x + 100.0 && first.label.x + first.label.w <= l.nodes[1].x
        );
    }

    #[test]
    fn a_long_edge_routes_through_the_layers_it_skips() {
        let l = layout(
            &[node(100.0, true), node(100.0, false), node(100.0, false)],
            &[edge(0, 1), edge(1, 2), edge(0, 2)],
            M,
        );
        let long = &l.edges[2];
        assert!(
            long.points
                .last()
                .is_some_and(|p| (p.0 - l.nodes[2].x).abs() < 0.01),
            "ends on the target's west side"
        );
        assert!(long.points.len() > 2);
        let mid_col = l.nodes[1].x;
        assert!(
            long.points
                .iter()
                .any(|p| p.0 > mid_col && p.0 < mid_col + 100.0 + 1.0),
            "passes the middle layer's column"
        );
    }

    #[test]
    fn rtl_mirrors_the_whole_layout_and_empty_input_is_safe() {
        let nodes = [node(100.0, true), node(100.0, false)];
        let ltr = layout(&nodes, &[edge(0, 1)], M);
        let rtl = layout(&nodes, &[edge(0, 1)], Metrics { rtl: true, ..M });
        assert_eq!(ltr.width, rtl.width);
        assert!((rtl.nodes[0].x - (ltr.width - (ltr.nodes[0].x + 100.0))).abs() < 0.01);
        assert!(rtl.nodes[0].x > rtl.nodes[1].x, "flow runs right to left");
        assert_eq!(layout(&[], &[], M).width, 0.0);
    }

    #[test]
    fn isotonic_placement_keeps_order_and_separation_and_moves_least() {
        let y = isotonic(&[10.0, 10.0, 10.0], &[20.0, 20.0]);
        assert_eq!(
            y,
            vec![-10.0, 10.0, 30.0],
            "an over-tight trio spreads symmetrically about its target"
        );
        let free = isotonic(&[0.0, 100.0], &[20.0]);
        assert_eq!(
            free,
            vec![0.0, 100.0],
            "control: a pair already apart does not move"
        );
    }
}
