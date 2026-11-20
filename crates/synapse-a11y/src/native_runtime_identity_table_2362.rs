//! FROZEN DRAFT #2362(a). Manager approves table, installs, records red, then freezes.
//! Codex and completion pass never edit this file.
use super::NativeRuntimeIdentity;

#[derive(Clone, Copy)]
struct Input<'a> {
    hwnd: i64,
    runtime: &'a str,
    pid: i32,
    role: &'a str,
    class: &'a str,
    automation: &'a str,
    name: &'a str,
}

const BASE: Input<'static> = Input {
    hwnd: 0x4c0590,
    runtime: "0000002a00190692000000040000000400000001000000fc",
    pid: 18768,
    role: "Group",
    class: "Chrome_RenderWidgetHostHWND",
    automation: "",
    name: "hello.txt",
};

fn token(input: Input<'_>) -> Result<String, &'static str> {
    NativeRuntimeIdentity::new(
        input.hwnd,
        input.runtime,
        input.pid,
        input.role,
        input.class,
        input.automation,
        input.name,
    )
    .map(|identity| identity.runtime_id_hex())
}

fn bound(input: Input<'_>) -> String {
    let value = match token(input) {
        Ok(value) => value,
        Err(error) => panic!("valid identity refused: {error}"),
    };
    assert_eq!(
        value.len(),
        72,
        "identity must have version marker plus full SHA-256"
    );
    assert!(
        value.starts_with("eded0001"),
        "identity is not bound to the observed item"
    );
    assert!(value.bytes().all(|byte| byte.is_ascii_hexdigit()));
    value
}

fn different(input: Input<'_>) {
    assert_ne!(
        token(BASE),
        token(input),
        "changed item must not keep its old id"
    );
    let original = bound(BASE);
    let _ = bound(input);
    assert_eq!(original, bound(BASE), "unchanged control must keep its id");
}

macro_rules! row {
    ($name:ident, $body:block) => {
        #[test]
        fn $name() $body
    };
}

row!(i01_normal_and_runtime_hex_case, {
    let first = bound(BASE);
    assert_eq!(
        first,
        bound(Input {
            runtime: "0000002A00190692000000040000000400000001000000FC",
            ..BASE
        })
    );
    different(Input {
        name: "other.txt",
        ..BASE
    });
});
row!(i02_reused_runtime_for_another_name, {
    different(Input {
        name: "lines300.txt",
        ..BASE
    });
});
row!(i03_changed_role, {
    different(Input {
        role: "Button",
        ..BASE
    });
});
row!(i04_changed_automation_id, {
    different(Input {
        automation: "another-item",
        ..BASE
    });
});
row!(i05_changed_class, {
    different(Input {
        class: "another-class",
        ..BASE
    });
});
row!(i06_second_process, {
    different(Input { pid: 18769, ..BASE });
});
row!(i07_different_runtime_same_name, {
    different(Input {
        runtime: "0000002a0000041a",
        ..BASE
    });
});
row!(i08_empty_and_hostile_runtime, {
    assert_eq!(
        token(Input {
            runtime: "",
            ..BASE
        }),
        Err("runtime id must be non-empty hex")
    );
    assert_eq!(
        token(Input {
            runtime: "aa:bb",
            ..BASE
        }),
        Err("runtime id must be non-empty hex")
    );
    assert_eq!(
        token(Input {
            runtime: "ab\0cd",
            ..BASE
        }),
        Err("runtime id must be non-empty hex")
    );
    let _ = bound(BASE);
});
row!(i09_unknown_process, {
    assert_eq!(
        token(Input { pid: 0, ..BASE }),
        Err("process id must be positive")
    );
    assert_eq!(
        token(Input { pid: -1, ..BASE }),
        Err("process id must be positive")
    );
    let _ = bound(BASE);
});
row!(i10_missing_role, {
    assert_eq!(
        token(Input { role: "", ..BASE }),
        Err("role must be non-empty")
    );
    let _ = bound(BASE);
});
row!(i11_unnamed_control, {
    let input = Input { name: "", ..BASE };
    assert_ne!(token(input), token(BASE));
    assert_eq!(bound(input), bound(input));
});
row!(i12_structured_and_unicode_names, {
    different(Input {
        name: "\u{03bb}\u{1f642}\0:hello.txt",
        ..BASE
    });
    // Field boundaries must be unambiguous, even with separator-like input.
    assert_ne!(
        bound(Input {
            automation: "ab",
            name: "c",
            ..BASE
        }),
        bound(Input {
            automation: "a",
            name: "bc",
            ..BASE
        }),
    );
    assert_ne!(
        bound(Input {
            name: "Hello.txt",
            ..BASE
        }),
        bound(BASE)
    );
});
row!(i13_recreate_identity_after_daemon_restart, {
    assert_eq!(bound(BASE), bound(BASE)); // two independent constructor calls
    different(Input { pid: 18769, ..BASE });
});
row!(i14_someone_elses_window, {
    different(Input {
        hwnd: 0x4c0591,
        ..BASE
    });
});
row!(i15_large_and_minimal_inputs, {
    let name = "x".repeat(65536);
    let long = Input {
        name: &name,
        ..BASE
    };
    assert_eq!(bound(long).len(), 72);
    assert_ne!(bound(long), bound(BASE));
    let minimal = Input {
        hwnd: 0,
        runtime: "0",
        pid: 1,
        role: "x",
        class: "",
        automation: "",
        name: "",
    };
    assert_eq!(bound(minimal), bound(minimal));
    assert_ne!(bound(minimal), bound(BASE));
});
