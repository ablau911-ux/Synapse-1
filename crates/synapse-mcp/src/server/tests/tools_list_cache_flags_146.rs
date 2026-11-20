//! #146: the process-wide tools/list cache must not serve one flag combination's
//! list to a service built under another (SYNAPSE_DEBUG_TOOLS /
//! SYNAPSE_ENABLE_EVERQUEST change the registered routes).
use std::sync::Arc;

use rmcp::model::Tool;

use crate::server::tools_list_cache::{ToolListFlags, ToolsListCache};

fn tool(name: &str) -> Tool {
    Tool::new(name.to_owned(), "test", Arc::new(serde_json::Map::new()))
}

fn flags(debug_tools: bool, everquest: bool) -> ToolListFlags {
    ToolListFlags {
        debug_tools,
        everquest,
    }
}

#[test]
fn must_pass_every_flag_combination_has_its_own_slot() {
    let cache = ToolsListCache::new();
    let combos = [(false, false), (true, false), (false, true), (true, true)];
    for (debug, eq) in combos {
        let name = format!("tool_{debug}_{eq}");
        let list = cache.get_or_compute(flags(debug, eq), || vec![tool(&name)]);
        assert_eq!(&*list[0].name, name);
    }
    for (debug, eq) in combos {
        let list = cache.get_or_compute(flags(debug, eq), Vec::new);
        assert_eq!(&*list[0].name, format!("tool_{debug}_{eq}"));
    }
}

#[test]
fn must_fail_different_flags_never_share_an_allocation() {
    let cache = ToolsListCache::new();
    let plain = cache.get_or_compute(flags(false, false), || vec![tool("a")]);
    let debug = cache.get_or_compute(flags(true, false), || vec![tool("a"), tool("fault")]);
    assert!(!Arc::ptr_eq(&plain, &debug));
    assert_eq!((plain.len(), debug.len()), (1, 2));
}
