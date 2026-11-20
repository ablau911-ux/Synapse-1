//! #107 part (d): which control a background point click acts on, and how the
//! click is delivered (roadmap item #568). Pure decisions; the OS reads and the
//! delivery stay in `point_post`.

use synapse_core::{AccessibleNode, ElementId, MouseButton, Point, UiaPattern};

#[cfg(test)]
#[path = "point_element_table_107d.rs"]
#[allow(clippy::type_complexity)]
mod point_element_table_107d;

/// The UIA patterns that click a control (the chain `click_element_action` runs).
const CLICK_PATTERNS: [UiaPattern; 4] = [
    UiaPattern::Invoke,
    UiaPattern::Toggle,
    UiaPattern::SelectionItem,
    UiaPattern::ExpandCollapse,
];

/// UIA `FrameworkId`s of frameworks that draw their controls themselves and
/// ignore posted mouse messages (compared in any letter case).
const WINDOWLESS_FRAMEWORKS: [&str; 2] = ["WPF", "XAML"];

/// The control at the point: its element id, the framework that draws it
/// (UIA `FrameworkId`, e.g. `WPF`, `XAML`, `WinForm`, `Win32`) and the UIA
/// patterns it offers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PointElement {
    element_id: ElementId,
    framework: String,
    patterns: Vec<UiaPattern>,
}

impl PointElement {
    /// Takes the picked node's id and patterns and the framework read for it.
    pub(crate) const fn new(
        element_id: ElementId,
        framework: String,
        patterns: Vec<UiaPattern>,
    ) -> Self {
        Self {
            element_id,
            framework,
            patterns,
        }
    }

    /// The element the UIA pattern click goes to.
    pub(crate) const fn element_id(&self) -> &ElementId {
        &self.element_id
    }

    /// Drawn by WPF or XAML, which ignore posted mouse messages.
    fn is_windowless(&self) -> bool {
        WINDOWLESS_FRAMEWORKS
            .iter()
            .any(|framework| self.framework.eq_ignore_ascii_case(framework))
    }

    fn offers_click_pattern(&self) -> bool {
        Self::click_pattern_in(&self.patterns)
    }

    /// The one place that says which patterns click a control.
    fn click_pattern_in(patterns: &[UiaPattern]) -> bool {
        patterns
            .iter()
            .any(|pattern| CLICK_PATTERNS.contains(pattern))
    }
}

/// Finds the control at a point among the accessibility nodes of the claimed
/// window.
pub(crate) struct PointElementPicker {
    nodes: Vec<AccessibleNode>,
}

impl PointElementPicker {
    /// Takes the nodes of the claimed window (`AccessibleSubtree::nodes`).
    pub(crate) const fn new(nodes: Vec<AccessibleNode>) -> Self {
        Self { nodes }
    }

    /// The control at `point` (screen physical pixels, like the node boxes):
    /// the deepest node whose box holds the point (on a tie the smallest box),
    /// or its nearest ancestor that holds the point and offers a click
    /// pattern. A disabled node on that way gives `None`: a disabled button is
    /// never clicked through its parent.
    pub(crate) fn pick(&self, point: Point) -> Option<&AccessibleNode> {
        let mut current = self
            .nodes
            .iter()
            .filter(|node| node.bbox.contains(point))
            .min_by_key(|node| (std::cmp::Reverse(node.depth), Self::area(node)))?;
        // One step per node: a broken parent chain (a cycle) cannot loop.
        for _ in 0..self.nodes.len() {
            if !current.enabled {
                return None;
            }
            if current.bbox.contains(point) && PointElement::click_pattern_in(&current.patterns) {
                return Some(current);
            }
            let parent = current.parent.as_ref()?;
            current = self.nodes.iter().find(|node| &node.element_id == parent)?;
        }
        None
    }

    fn area(node: &AccessibleNode) -> i64 {
        i64::from(node.bbox.w) * i64::from(node.bbox.h)
    }
}

/// The ONE delivery of a background point click on a claimed window.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum PointClickPlan {
    /// `BM_CLICK` to the classic button under the point (as before #107 d).
    BmClick,
    /// One UI Automation pattern click on this control; no message is posted.
    UiaPattern(PointElement),
    /// Posted mouse messages to the child window under the point (as before #107 d).
    PostedMessages,
}

impl PointClickPlan {
    /// Decides the delivery (first match wins) from the button, the click
    /// count, whether the child window under the point is a classic button,
    /// and the control at the point, which is read only when rules 1 and 2
    /// do not decide (a classic button or a double click costs no UIA read).
    pub(crate) fn choose(
        button: MouseButton,
        clicks: u8,
        hit_is_native_button: bool,
        element_at_point: impl FnOnce() -> Option<PointElement>,
    ) -> Self {
        if button != MouseButton::Left || clicks != 1 {
            return Self::PostedMessages;
        }
        if hit_is_native_button {
            return Self::BmClick;
        }
        match element_at_point() {
            Some(element) if element.is_windowless() && element.offers_click_pattern() => {
                Self::UiaPattern(element)
            }
            _ => Self::PostedMessages,
        }
    }
}
