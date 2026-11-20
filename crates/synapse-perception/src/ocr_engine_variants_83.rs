//! #83 profile engine: truthful null diagnostics and successful-only caching.
//! The owner guard couples injected cache tests to the production factory.
use super::*;
use std::cell::Cell;
use std::sync::OnceLock;

fn assert_profile_owner_uses_success_only_cache() {
    // The available seam cannot replace the owner's process-global WinRT factory.
    // Guard that delegation as well as exercising its injected failure/retry below.
    let source = include_str!("ocr.rs");
    let owner = source
        .split("fn ocr_engine()")
        .nth(1)
        .expect("production OCR engine owner")
        .split("fn successful_engine_with")
        .next()
        .expect("owner body");
    let owner: String = owner
        .lines()
        .map(|line| line.split("//").next().unwrap_or_default())
        .flat_map(str::chars)
        .filter(|ch| !ch.is_whitespace())
        .collect();
    assert!(
        owner.contains("staticENGINE:OnceLock<OcrEngine>=OnceLock::new();"),
        "#83: ocr_engine must cache successful engines, never Result errors"
    );
    assert!(owner.contains("successful_engine_with(&ENGINE,||{OcrEngine::TryCreateFromUserProfileLanguages().map_err(ocr_engine_creation_error)})"),
        "#83: real ocr_engine must use the failure/retry seam and truthful diagnostics");
}

#[test]
fn must_fail_engine_creation_does_not_cache_failure_and_next_request_retries() {
    assert_profile_owner_uses_success_only_cache();
    let cache = OnceLock::<u32>::new();
    let calls = Cell::new(0);
    assert_eq!(cache.get(), None);
    let error = successful_engine_with(&cache, || {
        calls.set(calls.get() + 1);
        Err(backend_unavailable("recognizer temporarily absent".into()))
    })
    .expect_err("first creation fails");
    assert_eq!(error.code(), "OCR_BACKEND_UNAVAILABLE");
    assert_eq!(cache.get(), None);
    assert_eq!(calls.get(), 1);
    let engine = successful_engine_with(&cache, || {
        calls.set(calls.get() + 1);
        Ok(83)
    })
    .expect("retry after capability becomes available");
    assert_eq!(*engine, 83);
    assert_eq!(cache.get(), Some(&83));
    assert_eq!(calls.get(), 2);
}

#[test]
fn must_pass_successful_engine_is_reused_without_another_creation() {
    assert_profile_owner_uses_success_only_cache();
    let cache = OnceLock::<u32>::new();
    assert_eq!(
        *successful_engine_with(&cache, || Ok(83)).expect("create"),
        83
    );
    assert_eq!(cache.get(), Some(&83));
    let calls = Cell::new(0);
    let reused = successful_engine_with(&cache, || {
        calls.set(calls.get() + 1);
        Err(backend_unavailable("must not recreate".into()))
    })
    .expect("cached engine");
    assert_eq!(*reused, 83);
    assert_eq!(calls.get(), 0);
}

#[test]
fn must_pass_real_profile_engine_owner_matches_winrt_capability_and_reuses_success() {
    assert_profile_owner_uses_success_only_cache();
    // Query the OS independently; never assume a recognizer is installed.
    match OcrEngine::TryCreateFromUserProfileLanguages() {
        Ok(_) => {
            let first = ocr_engine().expect("OS can create a profile recognizer");
            let second = ocr_engine().expect("owner reuses its successful engine");
            assert!(std::ptr::eq(first, second), "#83: owner must reuse success");
        }
        Err(raw) => {
            let primary = raw.to_string();
            for _ in 0..2 {
                let error = ocr_engine().expect_err("OS cannot create a profile recognizer");
                assert_eq!(error.code(), "OCR_BACKEND_UNAVAILABLE");
                let detail = error.to_string();
                if raw.code().is_err() {
                    assert!(detail.contains(&primary), "real HRESULT: {detail}");
                } else {
                    assert!(
                        detail.contains(
                            "no installed WinRT recognizer supports the user profile languages"
                        ),
                        "null recognizer: {detail}"
                    );
                    assert!(
                        !detail.contains("0x00000000"),
                        "#83: S_OK is not a failure cause"
                    );
                }
            }
        }
    }
}

#[test]
fn must_fail_profile_null_s_ok_names_missing_recognizer_and_installed_languages() {
    assert_profile_owner_uses_success_only_cache();
    let null = windows::core::Error::empty();
    assert_eq!(
        null.code().0,
        0,
        "precondition: null projection sentinel is S_OK"
    );
    let languages =
        OcrEngine::AvailableRecognizerLanguages().expect("independent installed-language query");
    let mut expected = (0..languages.Size().expect("language count"))
        .map(|i| {
            languages
                .GetAt(i)
                .expect("language")
                .LanguageTag()
                .expect("tag")
                .to_string_lossy()
        })
        .collect::<Vec<_>>();
    expected.sort_unstable();
    expected.dedup();
    let error = ocr_engine_creation_error(null);
    assert_eq!(error.code(), "OCR_BACKEND_UNAVAILABLE");
    let text = error.to_string();
    assert!(text.contains("no installed WinRT recognizer supports the user profile languages"));
    assert!(text.contains(&format!(
        "available recognizer languages: [{}]",
        expected.join(", ")
    )));
    assert!(!text.contains("operation completed successfully"));
    assert!(!text.contains("0x00000000"));
}

#[test]
fn must_fail_real_hresult_keeps_primary_error_instead_of_unsupported_language_claim() {
    assert_profile_owner_uses_success_only_cache();
    let raw = windows::core::Error::from_hresult(windows::core::HRESULT(0x80004005_u32 as i32));
    assert!(raw.code().is_err());
    let primary = raw.to_string();
    let error = ocr_engine_creation_error(raw);
    assert_eq!(error.code(), "OCR_BACKEND_UNAVAILABLE");
    assert!(error.to_string().contains(&primary));
    assert!(
        !error
            .to_string()
            .contains("no installed WinRT recognizer supports")
    );
}
