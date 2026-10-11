//! Deterministic static layouts for `graph-view` (flow / tree / radial) — the line-by-line Rust
//! port of `@almadar/ui` `lib/graphViewLayouts.ts`, plus the force simulation `GraphView.tsx`
//! runs for its default layout (its random seed jitter replaced by none, so a scene is stable).

use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutMode {
    Force,
    Flow,
    Tree,
    Radial,
}

impl LayoutMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "force" => Some(Self::Force),
            "flow" => Some(Self::Flow),
            "tree" => Some(Self::Tree),
            "radial" => Some(Self::Radial),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

struct Adjacency {
    out: Vec<Vec<usize>>,
    inc: Vec<Vec<usize>>,
}

fn build_adjacency(ids: &[String], edges: &[(String, String)]) -> Adjacency {
    let index: HashMap<&str, usize> = ids
        .iter()
        .enumerate()
        .map(|(i, id)| (id.as_str(), i))
        .collect();
    let mut out = vec![Vec::new(); ids.len()];
    let mut inc = vec![Vec::new(); ids.len()];
    for (s, t) in edges {
        if let (Some(&a), Some(&b)) = (index.get(s.as_str()), index.get(t.as_str())) {
            out[a].push(b);
            inc[b].push(a);
        }
    }
    Adjacency { out, inc }
}

fn find_roots(n: usize, adj: &Adjacency) -> Vec<usize> {
    (0..n).filter(|&i| adj.inc[i].is_empty()).collect()
}

fn assign_layers(n: usize, adj: &Adjacency, roots: &[usize]) -> Vec<usize> {
    fn visit(id: usize, adj: &Adjacency, layer: &mut Vec<usize>, on_stack: &mut HashSet<usize>) {
        on_stack.insert(id);
        let current = layer[id];
        for &next in &adj.out[id] {
            if on_stack.contains(&next) {
                continue;
            }
            if current + 1 > layer[next] {
                layer[next] = current + 1;
            }
            visit(next, adj, layer, on_stack);
        }
        on_stack.remove(&id);
    }
    let mut layer = vec![0; n];
    let mut on_stack = HashSet::new();
    for &r in roots {
        visit(r, adj, &mut layer, &mut on_stack);
    }
    layer
}

fn compute_visit_order(n: usize, adj: &Adjacency, roots: &[usize]) -> Vec<usize> {
    fn visit(id: usize, adj: &Adjacency, order: &mut Vec<Option<usize>>, counter: &mut usize) {
        if order[id].is_some() {
            return;
        }
        order[id] = Some(*counter);
        *counter += 1;
        for &next in &adj.out[id] {
            visit(next, adj, order, counter);
        }
    }
    let mut order = vec![None; n];
    let mut counter = 0;
    for &r in roots {
        visit(r, adj, &mut order, &mut counter);
    }
    for slot in order.iter_mut().filter(|o| o.is_none()) {
        *slot = Some(counter);
        counter += 1;
    }
    order.into_iter().map(|o| o.unwrap_or(0)).collect()
}

fn bfs_depth_and_order(n: usize, adj: &Adjacency, roots: &[usize]) -> (Vec<usize>, Vec<usize>) {
    let mut depth = vec![0; n];
    let mut order: Vec<Option<usize>> = vec![None; n];
    let mut visited = vec![false; n];
    let mut queue = Vec::new();
    let mut counter = 0;
    for &r in roots {
        if visited[r] {
            continue;
        }
        visited[r] = true;
        depth[r] = 0;
        order[r] = Some(counter);
        counter += 1;
        queue.push(r);
    }
    let mut head = 0;
    while head < queue.len() {
        let id = queue[head];
        head += 1;
        let d = depth[id];
        for &next in &adj.out[id] {
            if visited[next] {
                continue;
            }
            visited[next] = true;
            depth[next] = d + 1;
            order[next] = Some(counter);
            counter += 1;
            queue.push(next);
        }
    }
    for slot in order.iter_mut().filter(|o| o.is_none()) {
        *slot = Some(counter);
        counter += 1;
    }
    (depth, order.into_iter().map(|o| o.unwrap_or(0)).collect())
}

fn edge_walk_order(n: usize, adj: &Adjacency) -> Vec<usize> {
    let mut visited = vec![false; n];
    let mut result = Vec::new();
    for start in 0..n {
        if visited[start] {
            continue;
        }
        let mut current = Some(start);
        while let Some(c) = current {
            if visited[c] {
                break;
            }
            visited[c] = true;
            result.push(c);
            current = adj.out[c].iter().copied().find(|&next| !visited[next]);
        }
    }
    result
}

fn group_by_tier(n: usize, tier_of: &[usize], max_tier: usize) -> Vec<Vec<usize>> {
    let mut groups = vec![Vec::new(); max_tier + 1];
    for i in 0..n {
        groups[tier_of[i]].push(i);
    }
    groups
}

fn distribute_axis(count: usize, start: f32, end: f32) -> Vec<f32> {
    if count == 0 {
        return Vec::new();
    }
    let slot = (end - start) / count as f32;
    (0..count)
        .map(|i| start + slot * (i as f32 + 0.5))
        .collect()
}

fn order_by_parent_position(
    group: &[usize],
    adj: &Adjacency,
    positioned: &HashMap<usize, f32>,
) -> Vec<usize> {
    let mut keyed: Vec<(usize, f32)> = group
        .iter()
        .map(|&id| {
            let parents: Vec<f32> = adj.inc[id]
                .iter()
                .filter_map(|p| positioned.get(p).copied())
                .collect();
            let avg = if parents.is_empty() {
                f32::INFINITY
            } else {
                parents.iter().sum::<f32>() / parents.len() as f32
            };
            (id, avg)
        })
        .collect();
    keyed.sort_by(|a, b| {
        if a.1 != b.1 {
            a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal)
        } else {
            a.0.cmp(&b.0)
        }
    });
    keyed.into_iter().map(|(id, _)| id).collect()
}

fn layout_flow(n: usize, adj: &Adjacency, roots: &[usize], w: f32, h: f32, m: f32) -> Vec<Point> {
    let layers = assign_layers(n, adj, roots);
    let max_layer = layers.iter().copied().max().unwrap_or(0);
    let groups = group_by_tier(n, &layers, max_layer);
    let mut pos = vec![Point { x: 0.0, y: 0.0 }; n];
    let mut y_by_id: HashMap<usize, f32> = HashMap::new();
    for (l, group) in groups.iter().enumerate() {
        let x = m + (l as f32 * (w - 2.0 * m)) / max_layer.max(1) as f32;
        let ordered = order_by_parent_position(group, adj, &y_by_id);
        let ys = distribute_axis(ordered.len(), m, h - m);
        for (i, id) in ordered.into_iter().enumerate() {
            pos[id] = Point { x, y: ys[i] };
            y_by_id.insert(id, ys[i]);
        }
    }
    pos
}

fn layout_tree(n: usize, adj: &Adjacency, roots: &[usize], w: f32, h: f32, m: f32) -> Vec<Point> {
    let effective: Vec<usize> = if roots.is_empty() {
        vec![0]
    } else {
        roots.to_vec()
    };
    let layers = assign_layers(n, adj, &effective);
    let max_layer = layers.iter().copied().max().unwrap_or(0);
    let visit = compute_visit_order(n, adj, &effective);
    let groups = group_by_tier(n, &layers, max_layer);
    let mut pos = vec![Point { x: 0.0, y: 0.0 }; n];
    for (l, group) in groups.iter().enumerate() {
        let mut ordered = group.clone();
        ordered.sort_by_key(|&id| visit[id]);
        let y = m + (l as f32 * (h - 2.0 * m)) / max_layer.max(1) as f32;
        let xs = distribute_axis(ordered.len(), m, w - m);
        for (i, id) in ordered.into_iter().enumerate() {
            pos[id] = Point { x: xs[i], y };
        }
    }
    pos
}

fn layout_radial(n: usize, adj: &Adjacency, roots: &[usize], w: f32, h: f32, m: f32) -> Vec<Point> {
    let (cx, cy) = (w / 2.0, h / 2.0);
    let max_radius = w.min(h) / 2.0 - m;
    let mut pos = vec![Point { x: 0.0, y: 0.0 }; n];
    if roots.is_empty() {
        let order = edge_walk_order(n, adj);
        let k = order.len();
        for (i, id) in order.into_iter().enumerate() {
            let a = (i as f32 / k as f32) * std::f32::consts::TAU;
            pos[id] = Point {
                x: cx + max_radius * a.cos(),
                y: cy + max_radius * a.sin(),
            };
        }
        return pos;
    }
    let (depth, visit) = bfs_depth_and_order(n, adj, roots);
    let max_depth = depth.iter().copied().max().unwrap_or(0);
    let rings = group_by_tier(n, &depth, max_depth);
    for (d, ring) in rings.iter().enumerate() {
        let mut ordered = ring.clone();
        ordered.sort_by_key(|&id| visit[id]);
        let radius = (d as f32 * max_radius) / max_depth.max(1) as f32;
        let k = ordered.len();
        for (i, id) in ordered.into_iter().enumerate() {
            let a = (i as f32 / k as f32) * std::f32::consts::TAU;
            pos[id] = Point {
                x: cx + radius * a.cos(),
                y: cy + radius * a.sin(),
            };
        }
    }
    pos
}

/// `computeStaticLayout`: one point per node id, in input order.
pub fn static_layout(
    mode: LayoutMode,
    ids: &[String],
    edges: &[(String, String)],
    w: f32,
    h: f32,
) -> Vec<Point> {
    let margin = 40.0;
    match ids.len() {
        0 => return Vec::new(),
        1 => {
            return vec![Point {
                x: w / 2.0,
                y: h / 2.0,
            }]
        }
        _ => {}
    }
    let adj = build_adjacency(ids, edges);
    let roots = find_roots(ids.len(), &adj);
    match mode {
        LayoutMode::Flow => layout_flow(ids.len(), &adj, &roots, w, h, margin),
        LayoutMode::Tree => layout_tree(ids.len(), &adj, &roots, w, h, margin),
        LayoutMode::Radial => layout_radial(ids.len(), &adj, &roots, w, h, margin),
        LayoutMode::Force => unreachable!("force is simulated, not a static layout"),
    }
}

/// The 120-iteration force simulation `GraphView.tsx` runs: pairwise repulsion, edge springs
/// (rest 100), centre gravity, 0.85 damping, 40px wall margin. Nodes start on a ring of radius
/// `min(w, h) * 0.3` (the web adds a random +-10px jitter; none here).
pub fn force_layout(ids: &[String], edges: &[(String, String)], w: f32, h: f32) -> Vec<Point> {
    let n = ids.len();
    let mut pos: Vec<Point> = (0..n)
        .map(|i| {
            let a = i as f32 / n as f32 * std::f32::consts::TAU;
            let r = w.min(h) * 0.3;
            Point {
                x: w / 2.0 + r * a.cos(),
                y: h / 2.0 + r * a.sin(),
            }
        })
        .collect();
    let mut vel = vec![(0.0f32, 0.0f32); n];
    let index: HashMap<&str, usize> = ids
        .iter()
        .enumerate()
        .map(|(i, id)| (id.as_str(), i))
        .collect();
    let links: Vec<(usize, usize)> = edges
        .iter()
        .filter_map(|(s, t)| Some((*index.get(s.as_str())?, *index.get(t.as_str())?)))
        .collect();
    for _ in 0..120 {
        let mut f = vec![(0.0f32, 0.0f32); n];
        for i in 0..n {
            for j in i + 1..n {
                let (dx, dy) = (pos[j].x - pos[i].x, pos[j].y - pos[i].y);
                let dist = (dx * dx + dy * dy).sqrt();
                let dist = if dist == 0.0 { 1.0 } else { dist };
                let force = 800.0 / (dist * dist);
                let (fx, fy) = (dx / dist * force, dy / dist * force);
                f[i].0 -= fx;
                f[i].1 -= fy;
                f[j].0 += fx;
                f[j].1 += fy;
            }
        }
        for &(s, t) in &links {
            let (dx, dy) = (pos[t].x - pos[s].x, pos[t].y - pos[s].y);
            let dist = (dx * dx + dy * dy).sqrt();
            let dist = if dist == 0.0 { 1.0 } else { dist };
            let force = (dist - 100.0) * 0.05;
            let (fx, fy) = (dx / dist * force, dy / dist * force);
            f[s].0 += fx;
            f[s].1 += fy;
            f[t].0 -= fx;
            f[t].1 -= fy;
        }
        for i in 0..n {
            f[i].0 += (w / 2.0 - pos[i].x) * 0.01;
            f[i].1 += (h / 2.0 - pos[i].y) * 0.01;
        }
        let margin = 40.0;
        for i in 0..n {
            vel[i].0 = (vel[i].0 + f[i].0) * 0.85;
            vel[i].1 = (vel[i].1 + f[i].1) * 0.85;
            pos[i].x = (pos[i].x + vel[i].0).clamp(margin, (w - margin).max(margin));
            pos[i].y = (pos[i].y + vel[i].1).clamp(margin, (h - margin).max(margin));
        }
    }
    pos
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    fn edges(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect()
    }

    #[test]
    fn flow_layers_a_dag_left_to_right_by_longest_path() {
        let n = ids(&["a", "b", "c", "d"]);
        let e = edges(&[("a", "b"), ("b", "c"), ("a", "c"), ("a", "d")]);
        let p = static_layout(LayoutMode::Flow, &n, &e, 400.0, 300.0);
        assert_eq!(p[0].x, 40.0, "root at the left margin");
        assert_eq!(p[1].x, 200.0);
        assert_eq!(
            p[2].x, 360.0,
            "c sits two layers deep although a->c is direct"
        );
        assert_eq!(p[3].x, 200.0, "d is one layer deep");
        assert!(
            p[1].y < p[3].y,
            "same layer keeps input order top to bottom"
        );
    }

    #[test]
    fn tree_stacks_tiers_top_down_and_a_pure_cycle_falls_back_to_the_first_node() {
        let n = ids(&["r", "x", "y"]);
        let e = edges(&[("r", "x"), ("r", "y")]);
        let p = static_layout(LayoutMode::Tree, &n, &e, 400.0, 300.0);
        assert_eq!(p[0].y, 40.0);
        assert_eq!(p[1].y, 260.0);
        assert!(p[1].x < p[2].x);
        let cyc = static_layout(
            LayoutMode::Tree,
            &ids(&["a", "b"]),
            &edges(&[("a", "b"), ("b", "a")]),
            400.0,
            300.0,
        );
        assert_eq!(cyc[0].y, 40.0, "no roots: the first node roots the tree");
    }

    #[test]
    fn radial_puts_roots_at_the_centre_and_a_pure_cycle_on_one_ring() {
        let n = ids(&["r", "x", "y"]);
        let e = edges(&[("r", "x"), ("r", "y")]);
        let p = static_layout(LayoutMode::Radial, &n, &e, 400.0, 400.0);
        assert_eq!((p[0].x, p[0].y), (200.0, 200.0));
        let ring = ((p[1].x - 200.0).powi(2) + (p[1].y - 200.0).powi(2)).sqrt();
        assert!(
            (ring - 160.0).abs() < 0.01,
            "depth 1 of 1 sits on the outer ring"
        );
        let cyc = static_layout(
            LayoutMode::Radial,
            &ids(&["a", "b", "c"]),
            &edges(&[("a", "b"), ("b", "c"), ("c", "a")]),
            400.0,
            400.0,
        );
        for pt in cyc {
            let r = ((pt.x - 200.0).powi(2) + (pt.y - 200.0).powi(2)).sqrt();
            assert!((r - 160.0).abs() < 0.01);
        }
    }

    #[test]
    fn empty_and_single_node_graphs_and_unknown_edge_ids_are_safe() {
        assert!(static_layout(LayoutMode::Flow, &[], &[], 100.0, 100.0).is_empty());
        let one = static_layout(LayoutMode::Radial, &ids(&["a"]), &[], 100.0, 80.0);
        assert_eq!(one, vec![Point { x: 50.0, y: 40.0 }]);
        let p = static_layout(
            LayoutMode::Flow,
            &ids(&["a", "b"]),
            &edges(&[("a", "ghost")]),
            400.0,
            300.0,
        );
        assert_eq!(p[0].x, p[1].x, "an edge to an unknown id adds no layer");
    }

    #[test]
    fn force_is_deterministic_stays_inside_the_wall_margin_and_springs_hold_linked_nodes_apart() {
        let n = ids(&["a", "b", "c"]);
        let linked = force_layout(&n, &edges(&[("a", "b")]), 600.0, 400.0);
        assert_eq!(
            linked,
            force_layout(&n, &edges(&[("a", "b")]), 600.0, 400.0)
        );
        for p in &linked {
            assert!(p.x >= 40.0 && p.x <= 560.0 && p.y >= 40.0 && p.y <= 360.0);
        }
        let apart = force_layout(&n, &[], 600.0, 400.0);
        let d = |p: &[Point]| ((p[0].x - p[1].x).powi(2) + (p[0].y - p[1].y).powi(2)).sqrt();
        assert!(
            (d(&linked) - 100.0).abs() < 25.0,
            "the spring rests near 100px: {}",
            d(&linked)
        );
        assert!(
            d(&apart) < d(&linked),
            "control: without the edge, gravity pulls the pair together"
        );
    }
}
