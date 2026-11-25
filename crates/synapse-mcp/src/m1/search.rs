use std::collections::HashMap;

use synapse_core::{AccessibleNode, DetectedEntity, ElementId};

use crate::m1::{FindParams, FindResult, FindResultKind};

/// The `find` `in_window` scope: every snapshot node below the named element,
/// at any depth (not only its direct children).
pub struct InWindowScope<'a> {
    root: &'a ElementId,
    parents: HashMap<&'a ElementId, Option<&'a ElementId>>,
}

impl<'a> InWindowScope<'a> {
    /// `None` when `root` is not a node of `elements`: the id is bogus, stale,
    /// foreign or deeper than the snapshot, so no scope can be drawn.
    #[must_use]
    pub fn new(elements: &'a [AccessibleNode], root: &'a ElementId) -> Option<Self> {
        let parents: HashMap<_, _> = elements
            .iter()
            .map(|node| (&node.element_id, node.parent.as_ref()))
            .collect();
        parents.contains_key(root).then_some(Self { root, parents })
    }

    /// True when `node` sits below the scope root. The walk is bounded by the
    /// node count, so a malformed parent chain cannot loop.
    #[must_use]
    pub fn contains(&self, node: &AccessibleNode) -> bool {
        let mut parent = node.parent.as_ref();
        for _ in 0..=self.parents.len() {
            match parent {
                Some(id) if id == self.root => return true,
                Some(id) => parent = self.parents.get(id).copied().flatten(),
                None => return false,
            }
        }
        false
    }
}

pub fn element_match(node: &AccessibleNode, params: &FindParams) -> Option<FindResult> {
    if let Some(role) = &params.role
        && !node.role.eq_ignore_ascii_case(role)
    {
        return None;
    }
    if let Some(name_substring) = &params.name_substring
        && !contains_ascii_case(&node.name, name_substring)
    {
        return None;
    }
    if let Some(automation_id) = &params.automation_id
        && node.automation_id.as_deref() != Some(automation_id.as_str())
    {
        return None;
    }
    let mut score = 0.25;
    if let Some(query) = &params.query {
        score += element_query_score(node, query)?;
    }
    if node.focused {
        score += 0.1;
    }
    if node.bbox.w > 0 && node.bbox.h > 0 {
        score += 0.15;
    } else {
        score -= 0.25;
    }
    if synapse_a11y::cdp_backend_from_element_id(&node.element_id).is_some() {
        score += 0.05;
    }
    Some(FindResult {
        kind: FindResultKind::Element,
        element_id: Some(node.element_id.clone()),
        entity_id: None,
        name: Some(node.name.clone()),
        role: Some(node.role.clone()),
        automation_id: node.automation_id.clone(),
        class_label: None,
        bbox: node.bbox,
        score,
    })
}

pub fn entity_match(entity: &DetectedEntity, params: &FindParams) -> Option<FindResult> {
    let query = params.query.as_ref()?;
    contains_ascii_case(&entity.class_label, query).then_some(FindResult {
        kind: FindResultKind::Entity,
        element_id: None,
        entity_id: Some(entity.entity_id.clone()),
        name: None,
        role: None,
        automation_id: None,
        class_label: Some(entity.class_label.clone()),
        bbox: entity.bbox,
        score: entity.confidence,
    })
}

fn contains_ascii_case(value: &str, needle: &str) -> bool {
    value
        .to_ascii_lowercase()
        .contains(&needle.to_ascii_lowercase())
}

fn element_query_score(node: &AccessibleNode, query: &str) -> Option<f32> {
    let query = query.trim();
    if query.is_empty() {
        return Some(0.0);
    }

    if contains_ascii_case(&node.name, query)
        || contains_ascii_case(&node.role, query)
        || node
            .automation_id
            .as_deref()
            .is_some_and(|value| contains_ascii_case(value, query))
    {
        return Some(0.65);
    }

    let terms = query_terms(query);
    if terms.is_empty() {
        return None;
    }
    let haystack = format!(
        "{} {} {}",
        node.name,
        node.role,
        node.automation_id.as_deref().unwrap_or_default()
    )
    .to_ascii_lowercase();
    let matched = terms
        .iter()
        .filter(|term| haystack.contains(term.as_str()))
        .count();
    if matched == 0 {
        return None;
    }

    let coverage = matched as f32 / terms.len() as f32;
    let density = matched.min(3) as f32 / 3.0;
    Some(0.35f32.mul_add(coverage, 0.15 * density))
}

fn query_terms(query: &str) -> Vec<String> {
    let mut terms = Vec::new();
    for raw in query
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|term| term.len() >= 2)
    {
        let term = raw.to_ascii_lowercase();
        if !terms.contains(&term) {
            terms.push(term);
        }
    }
    terms
}
