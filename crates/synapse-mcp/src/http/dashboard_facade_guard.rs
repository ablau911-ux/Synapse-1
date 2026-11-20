use std::collections::BTreeSet;

#[derive(Clone, Copy)]
pub enum DashboardFacade {
    Workspace,
    Agent,
    Approval,
    Task,
    Model,
    Hygiene,
}

impl DashboardFacade {
    pub(crate) const fn public_tool_name(self) -> &'static str {
        match self {
            Self::Workspace => "workspace",
            Self::Agent => "agent",
            Self::Approval => "approval",
            Self::Task => "task",
            Self::Model => "model",
            Self::Hygiene => "hygiene",
        }
    }

    pub(crate) fn available(self, tool_names: &BTreeSet<&str>) -> bool {
        tool_names.contains(self.public_tool_name())
    }
}
