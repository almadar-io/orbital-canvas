//! d3-quadtree (`addAll`, `cover`, `visit`, `visitAfter`), ported operation for operation so the
//! graph-canvas forces built on it visit nodes, and draw jiggle, in d3's order.

/// One quadrant: four children, or a leaf holding a point with any coincident points chained on.
pub enum Kind {
    Internal([Option<usize>; 4]),
    Leaf { data: usize, next: Option<usize> },
}

pub struct Quad {
    pub kind: Kind,
    /// Accumulator fields a force fills in `visit_after` (d3 stores them on the quad object).
    pub value: f64,
    pub x: f64,
    pub y: f64,
}

pub struct Quadtree {
    pub quads: Vec<Quad>,
    pub root: Option<usize>,
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
}

impl Quadtree {
    /// `quadtree(nodes, x, y)`: the extent of every valid point, then each point in order.
    pub fn build(points: &[(f64, f64)]) -> Self {
        let mut tree = Quadtree {
            quads: Vec::new(),
            root: None,
            x0: f64::NAN,
            y0: f64::NAN,
            x1: f64::NAN,
            y1: f64::NAN,
        };
        let (mut x0, mut y0, mut x1, mut y1) = (
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        );
        for &(x, y) in points {
            if x.is_nan() || y.is_nan() {
                continue;
            }
            x0 = x0.min(x);
            x1 = x1.max(x);
            y0 = y0.min(y);
            y1 = y1.max(y);
        }
        if x0 > x1 || y0 > y1 {
            return tree;
        }
        tree.cover(x0, y0);
        tree.cover(x1, y1);
        for (i, &(x, y)) in points.iter().enumerate() {
            tree.add(x, y, i, points);
        }
        tree
    }

    fn push(&mut self, kind: Kind) -> usize {
        self.quads.push(Quad {
            kind,
            value: 0.0,
            x: 0.0,
            y: 0.0,
        });
        self.quads.len() - 1
    }

    fn internal() -> Kind {
        Kind::Internal([None; 4])
    }

    fn set_child(&mut self, parent: usize, i: usize, child: usize) {
        if let Kind::Internal(c) = &mut self.quads[parent].kind {
            c[i] = Some(child);
        }
    }

    fn cover(&mut self, x: f64, y: f64) {
        let (mut x0, mut y0, mut x1, mut y1) = (self.x0, self.y0, self.x1, self.y1);
        if x0.is_nan() {
            x0 = x.floor();
            x1 = x0 + 1.0;
            y0 = y.floor();
            y1 = y0 + 1.0;
        } else {
            let mut z = if x1 - x0 != 0.0 { x1 - x0 } else { 1.0 };
            let mut node = self.root;
            while x0 > x || x >= x1 || y0 > y || y >= y1 {
                let i = (usize::from(y < y0) << 1) | usize::from(x < x0);
                let parent = self.push(Self::internal());
                if let Some(n) = node {
                    self.set_child(parent, i, n);
                }
                node = Some(parent);
                z *= 2.0;
                match i {
                    0 => {
                        x1 = x0 + z;
                        y1 = y0 + z;
                    }
                    1 => {
                        x0 = x1 - z;
                        y1 = y0 + z;
                    }
                    2 => {
                        x1 = x0 + z;
                        y0 = y1 - z;
                    }
                    _ => {
                        x0 = x1 - z;
                        y0 = y1 - z;
                    }
                }
            }
            if self
                .root
                .is_some_and(|r| matches!(self.quads[r].kind, Kind::Internal(_)))
            {
                self.root = node;
            }
        }
        (self.x0, self.y0, self.x1, self.y1) = (x0, y0, x1, y1);
    }

    fn add(&mut self, x: f64, y: f64, d: usize, points: &[(f64, f64)]) {
        if x.is_nan() || y.is_nan() {
            return;
        }
        let leaf = self.push(Kind::Leaf {
            data: d,
            next: None,
        });
        let Some(mut node) = self.root else {
            self.root = Some(leaf);
            return;
        };
        let (mut x0, mut y0, mut x1, mut y1) = (self.x0, self.y0, self.x1, self.y1);
        let mut parent: Option<(usize, usize)> = None;
        while let Kind::Internal(children) = self.quads[node].kind {
            let xm = (x0 + x1) / 2.0;
            let right = x >= xm;
            if right {
                x0 = xm
            } else {
                x1 = xm
            }
            let ym = (y0 + y1) / 2.0;
            let bottom = y >= ym;
            if bottom {
                y0 = ym
            } else {
                y1 = ym
            }
            let i = (usize::from(bottom) << 1) | usize::from(right);
            parent = Some((node, i));
            match children[i] {
                Some(c) => node = c,
                None => {
                    self.set_child(node, i, leaf);
                    return;
                }
            }
        }
        let Kind::Leaf { data: existing, .. } = self.quads[node].kind else {
            return;
        };
        let (xp, yp) = points[existing];
        if x == xp && y == yp {
            if let Kind::Leaf { next, .. } = &mut self.quads[leaf].kind {
                *next = Some(node);
            }
            match parent {
                Some((p, i)) => self.set_child(p, i, leaf),
                None => self.root = Some(leaf),
            }
            return;
        }
        let mut slot = parent;
        loop {
            let fresh = self.push(Self::internal());
            match slot {
                Some((p, i)) => self.set_child(p, i, fresh),
                None => self.root = Some(fresh),
            }
            let xm = (x0 + x1) / 2.0;
            let right = x >= xm;
            if right {
                x0 = xm
            } else {
                x1 = xm
            }
            let ym = (y0 + y1) / 2.0;
            let bottom = y >= ym;
            if bottom {
                y0 = ym
            } else {
                y1 = ym
            }
            let i = (usize::from(bottom) << 1) | usize::from(right);
            let j = (usize::from(yp >= ym) << 1) | usize::from(xp >= xm);
            if i != j {
                self.set_child(fresh, j, node);
                self.set_child(fresh, i, leaf);
                return;
            }
            slot = Some((fresh, i));
        }
    }

    fn children(&self, q: usize) -> Option<[Option<usize>; 4]> {
        match self.quads[q].kind {
            Kind::Internal(c) => Some(c),
            Kind::Leaf { .. } => None,
        }
    }

    /// `visitAfter`: every quad after its children, in d3's order.
    pub fn visit_after(&mut self, mut callback: impl FnMut(&mut Self, usize)) {
        let mut stack = Vec::new();
        let mut order = Vec::new();
        if let Some(r) = self.root {
            stack.push(r);
        }
        while let Some(q) = stack.pop() {
            if let Some(c) = self.children(q) {
                stack.extend(c.iter().flatten());
            }
            order.push(q);
        }
        while let Some(q) = order.pop() {
            callback(self, q);
        }
    }

    /// `visit`: pre-order; the callback gets the quad and its extent and returns true to skip
    /// the quad's children.
    pub fn visit(&self, mut callback: impl FnMut(usize, f64, f64, f64, f64) -> bool) {
        let mut stack = Vec::new();
        if let Some(r) = self.root {
            stack.push((r, self.x0, self.y0, self.x1, self.y1));
        }
        while let Some((q, x0, y0, x1, y1)) = stack.pop() {
            if callback(q, x0, y0, x1, y1) {
                continue;
            }
            if let Some(c) = self.children(q) {
                let (xm, ym) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
                if let Some(n) = c[3] {
                    stack.push((n, xm, ym, x1, y1));
                }
                if let Some(n) = c[2] {
                    stack.push((n, x0, ym, xm, y1));
                }
                if let Some(n) = c[1] {
                    stack.push((n, xm, y0, x1, ym));
                }
                if let Some(n) = c[0] {
                    stack.push((n, x0, y0, xm, ym));
                }
            }
        }
    }
}
