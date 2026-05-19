// Copyright (c) The Move Contributors
// SPDX-License-Identifier: Apache-2.0

//! Inline let-bindings whose RHS is a single variable, when both names are used at most once.
//!
//! For a binding `let X = Y;` we fire when:
//!   - `Y` is read exactly once in the function body (the binding's own RHS) and never written;
//!   - `X` is read at most once on any execution path, never re-assigned, and not declared
//!     elsewhere.
//!
//! In that case the binding is dropped and the single use of `X` is rewritten to `Y` directly.
//! Multiple non-conflicting bindings can be processed in one pass; chains like
//! `let l20 = l3; let l3 = l5;` are resolved via transitive closure before substitution.

use std::collections::{BTreeMap, BTreeSet};

use crate::ast::{Exp, UnstructuredNode};

pub fn refine(exp: &mut Exp) -> bool {
    let counts = NameCounts::collect(exp);

    let mut candidates: Vec<(String, String)> = Vec::new();
    collect_candidates(exp, exp, &counts, &mut candidates);
    if candidates.is_empty() {
        return false;
    }

    let drop_set: BTreeSet<(String, String)> = candidates.iter().cloned().collect();
    let mut sub_map: BTreeMap<String, String> = candidates.into_iter().collect();
    transitive_closure(&mut sub_map);

    apply(exp, &drop_set, &sub_map);
    true
}

// -------------------------------------------------------------------------------------------------
// Name counts

#[derive(Default)]
struct NameCounts {
    reads: BTreeMap<String, usize>,
    assigns: BTreeMap<String, usize>,
    letbinds: BTreeMap<String, usize>,
    declares: BTreeMap<String, usize>,
    unpacks: BTreeMap<String, usize>,
}

impl NameCounts {
    fn collect(exp: &Exp) -> Self {
        let mut nc = NameCounts::default();
        nc.visit(exp);
        nc
    }

    fn bump(map: &mut BTreeMap<String, usize>, n: &str) {
        *map.entry(n.to_string()).or_insert(0) += 1;
    }

    fn visit(&mut self, exp: &Exp) {
        use Exp as E;
        match exp {
            E::Variable(n) => Self::bump(&mut self.reads, n),
            E::Assign(targets, rhs) => {
                for t in targets {
                    Self::bump(&mut self.assigns, t);
                }
                self.visit(rhs);
            }
            E::LetBind(targets, rhs) => {
                for t in targets {
                    Self::bump(&mut self.letbinds, t);
                }
                self.visit(rhs);
            }
            E::Declare(names) => {
                for n in names {
                    Self::bump(&mut self.declares, n);
                }
            }
            E::VecUnpack(names, e) => {
                for n in names {
                    Self::bump(&mut self.unpacks, n);
                }
                self.visit(e);
            }
            E::Unpack(_, fields, e) | E::UnpackVariant(_, _, fields, e) => {
                for (_, n) in fields {
                    Self::bump(&mut self.unpacks, n);
                }
                self.visit(e);
            }
            E::Match(subject, _, arms) => {
                self.visit(subject);
                for (_, fields, body) in arms {
                    for (_, n) in fields {
                        Self::bump(&mut self.unpacks, n);
                    }
                    self.visit(body);
                }
            }
            E::Switch(subject, _, arms) => {
                self.visit(subject);
                for (_, body) in arms {
                    self.visit(body);
                }
            }
            E::IfElse(c, t, alt) => {
                self.visit(c);
                self.visit(t);
                if let Some(a) = alt.as_ref().as_ref() {
                    self.visit(a);
                }
            }
            E::Seq(items) | E::Return(items) | E::Call(_, items) => {
                for i in items {
                    self.visit(i);
                }
            }
            E::Loop(_, b) => self.visit(b),
            E::While(_, c, b) => {
                self.visit(c);
                self.visit(b);
            }
            E::Abort(e) | E::Borrow(_, e) => self.visit(e),
            E::Primitive { args, .. } | E::Data { args, .. } => {
                for a in args {
                    self.visit(a);
                }
            }
            E::Break(_) | E::Continue(_) | E::Value(_) | E::Constant(_) => {}
            E::Unstructured(nodes) => {
                for n in nodes {
                    match n {
                        UnstructuredNode::Labeled(_, b) | UnstructuredNode::Statement(b) => {
                            self.visit(b);
                        }
                        UnstructuredNode::Goto(_) => {}
                    }
                }
            }
        }
    }

    fn reads(&self, n: &str) -> usize {
        *self.reads.get(n).unwrap_or(&0)
    }
    fn assigns(&self, n: &str) -> usize {
        *self.assigns.get(n).unwrap_or(&0)
    }
    fn letbinds(&self, n: &str) -> usize {
        *self.letbinds.get(n).unwrap_or(&0)
    }
    fn declares(&self, n: &str) -> usize {
        *self.declares.get(n).unwrap_or(&0)
    }
    fn unpacks(&self, n: &str) -> usize {
        *self.unpacks.get(n).unwrap_or(&0)
    }
}

// -------------------------------------------------------------------------------------------------
// Candidate collection

fn collect_candidates(exp: &Exp, root: &Exp, counts: &NameCounts, out: &mut Vec<(String, String)>) {
    use Exp as E;
    // Local check first, then recurse.
    if let E::LetBind(targets, rhs) = exp
        && targets.len() == 1
        && let E::Variable(y) = rhs.as_ref()
        && is_eligible(&targets[0], y, root, counts)
    {
        out.push((targets[0].clone(), y.clone()));
    }
    match exp {
        E::Variable(_)
        | E::Declare(_)
        | E::Value(_)
        | E::Constant(_)
        | E::Break(_)
        | E::Continue(_) => {}
        E::LetBind(_, e)
        | E::Assign(_, e)
        | E::Abort(e)
        | E::Borrow(_, e)
        | E::VecUnpack(_, e)
        | E::Unpack(_, _, e)
        | E::UnpackVariant(_, _, _, e) => collect_candidates(e, root, counts, out),
        E::Loop(_, b) => collect_candidates(b, root, counts, out),
        E::While(_, c, b) => {
            collect_candidates(c, root, counts, out);
            collect_candidates(b, root, counts, out);
        }
        E::IfElse(c, t, alt) => {
            collect_candidates(c, root, counts, out);
            collect_candidates(t, root, counts, out);
            if let Some(a) = alt.as_ref().as_ref() {
                collect_candidates(a, root, counts, out);
            }
        }
        E::Seq(items) | E::Return(items) | E::Call(_, items) => {
            for i in items {
                collect_candidates(i, root, counts, out);
            }
        }
        E::Switch(subject, _, arms) => {
            collect_candidates(subject, root, counts, out);
            for (_, b) in arms {
                collect_candidates(b, root, counts, out);
            }
        }
        E::Match(subject, _, arms) => {
            collect_candidates(subject, root, counts, out);
            for (_, _, b) in arms {
                collect_candidates(b, root, counts, out);
            }
        }
        E::Primitive { args, .. } | E::Data { args, .. } => {
            for a in args {
                collect_candidates(a, root, counts, out);
            }
        }
        E::Unstructured(nodes) => {
            for n in nodes {
                match n {
                    UnstructuredNode::Labeled(_, b) | UnstructuredNode::Statement(b) => {
                        collect_candidates(b, root, counts, out);
                    }
                    UnstructuredNode::Goto(_) => {}
                }
            }
        }
    }
}

fn is_eligible(x: &str, y: &str, root: &Exp, counts: &NameCounts) -> bool {
    if x == y {
        return false;
    }

    // Source `y`: exactly one read (this binding's RHS), never written.
    if counts.reads(y) != 1 || counts.assigns(y) != 0 {
        return false;
    }

    // Destination `x`: declared once via this binding, never written or re-introduced.
    if counts.assigns(x) != 0
        || counts.letbinds(x) != 1
        || counts.declares(x) != 0
        || counts.unpacks(x) != 0
    {
        return false;
    }

    // `x` must have at least one read (otherwise it's dead code, not our concern) and the
    // per-path read count cannot exceed one. When total reads is one the per-path bound is
    // trivially satisfied; only walk the tree when the total exceeds one.
    let total = counts.reads(x);
    if total == 0 {
        return false;
    }
    if total > 1 && max_uses_per_path(root, x) > 1 {
        return false;
    }
    true
}

/// Maximum reads of `name` along any single execution path through `exp`. Loop/while bodies
/// that contain any read return `usize::MAX` since the loop may iterate. Branches contribute
/// the max of their arms (only one runs), sequences sum (all run).
fn max_uses_per_path(exp: &Exp, name: &str) -> usize {
    use Exp as E;
    match exp {
        E::Variable(n) if n == name => 1,
        E::Variable(_)
        | E::Value(_)
        | E::Constant(_)
        | E::Break(_)
        | E::Continue(_)
        | E::Declare(_) => 0,
        E::Seq(items) | E::Return(items) | E::Call(_, items) => items
            .iter()
            .map(|i| max_uses_per_path(i, name))
            .fold(0, usize::saturating_add),
        E::IfElse(c, t, alt) => {
            let cc = max_uses_per_path(c, name);
            let tt = max_uses_per_path(t, name);
            let ee = alt
                .as_ref()
                .as_ref()
                .map(|a| max_uses_per_path(a, name))
                .unwrap_or(0);
            cc.saturating_add(tt.max(ee))
        }
        E::Switch(subject, _, arms) => {
            let sc = max_uses_per_path(subject, name);
            let m = arms
                .iter()
                .map(|(_, b)| max_uses_per_path(b, name))
                .max()
                .unwrap_or(0);
            sc.saturating_add(m)
        }
        E::Match(subject, _, arms) => {
            let sc = max_uses_per_path(subject, name);
            let m = arms
                .iter()
                .map(|(_, _, b)| max_uses_per_path(b, name))
                .max()
                .unwrap_or(0);
            sc.saturating_add(m)
        }
        E::Loop(_, b) => {
            if max_uses_per_path(b, name) > 0 {
                usize::MAX
            } else {
                0
            }
        }
        E::While(_, c, b) => {
            if max_uses_per_path(c, name) > 0 || max_uses_per_path(b, name) > 0 {
                usize::MAX
            } else {
                0
            }
        }
        E::Assign(_, e)
        | E::LetBind(_, e)
        | E::Abort(e)
        | E::Borrow(_, e)
        | E::VecUnpack(_, e)
        | E::Unpack(_, _, e)
        | E::UnpackVariant(_, _, _, e) => max_uses_per_path(e, name),
        E::Primitive { args, .. } | E::Data { args, .. } => args
            .iter()
            .map(|a| max_uses_per_path(a, name))
            .fold(0, usize::saturating_add),
        // Conservative: unstructured graphs may reach arbitrary points; treat as unbounded.
        E::Unstructured(_) => usize::MAX,
    }
}

// -------------------------------------------------------------------------------------------------
// Apply

/// Resolve each value to its furthest target so chains `(X -> Y), (Y -> Z)` collapse to
/// `(X -> Z), (Y -> Z)` before we substitute Variable nodes.
fn transitive_closure(subs: &mut BTreeMap<String, String>) {
    let keys: Vec<String> = subs.keys().cloned().collect();
    for k in keys {
        let mut cur = subs[&k].clone();
        while let Some(next) = subs.get(&cur) {
            if *next == cur {
                break;
            }
            cur = next.clone();
        }
        subs.insert(k, cur);
    }
}

fn apply(exp: &mut Exp, drop_set: &BTreeSet<(String, String)>, sub_map: &BTreeMap<String, String>) {
    use Exp as E;
    if let E::Seq(items) = exp {
        items.retain(|item| !is_dropped(item, drop_set));
    }
    if let E::Variable(n) = exp {
        if let Some(y) = sub_map.get(n.as_str()) {
            *n = y.clone();
        }
        return;
    }
    match exp {
        E::Variable(_)
        | E::Declare(_)
        | E::Value(_)
        | E::Constant(_)
        | E::Break(_)
        | E::Continue(_) => {}
        E::LetBind(_, e)
        | E::Assign(_, e)
        | E::Abort(e)
        | E::Borrow(_, e)
        | E::VecUnpack(_, e)
        | E::Unpack(_, _, e)
        | E::UnpackVariant(_, _, _, e) => apply(e, drop_set, sub_map),
        E::Loop(_, b) => apply(b, drop_set, sub_map),
        E::While(_, c, b) => {
            apply(c, drop_set, sub_map);
            apply(b, drop_set, sub_map);
        }
        E::IfElse(c, t, alt) => {
            apply(c, drop_set, sub_map);
            apply(t, drop_set, sub_map);
            if let Some(a) = alt.as_mut().as_mut() {
                apply(a, drop_set, sub_map);
            }
        }
        E::Seq(items) | E::Return(items) | E::Call(_, items) => {
            for i in items {
                apply(i, drop_set, sub_map);
            }
        }
        E::Switch(subject, _, arms) => {
            apply(subject, drop_set, sub_map);
            for (_, b) in arms {
                apply(b, drop_set, sub_map);
            }
        }
        E::Match(subject, _, arms) => {
            apply(subject, drop_set, sub_map);
            for (_, _, b) in arms {
                apply(b, drop_set, sub_map);
            }
        }
        E::Primitive { args, .. } | E::Data { args, .. } => {
            for a in args {
                apply(a, drop_set, sub_map);
            }
        }
        E::Unstructured(nodes) => {
            for n in nodes {
                match n {
                    UnstructuredNode::Labeled(_, b) | UnstructuredNode::Statement(b) => {
                        apply(b, drop_set, sub_map);
                    }
                    UnstructuredNode::Goto(_) => {}
                }
            }
        }
    }
}

fn is_dropped(item: &Exp, drop_set: &BTreeSet<(String, String)>) -> bool {
    if let Exp::LetBind(targets, rhs) = item
        && targets.len() == 1
        && let Exp::Variable(y) = rhs.as_ref()
    {
        drop_set.contains(&(targets[0].clone(), y.clone()))
    } else {
        false
    }
}
