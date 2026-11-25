# CMake toolchain file for every cmake-built -sys crate in this workspace, named in
# .cargo/config.toml [env] CMAKE_TOOLCHAIN_FILE (clip recorder plan 5.0, 2026-11-24).
#
# libaom-sys passes -DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded (static C runtime, /MT)
# on MSVC no matter what Rust uses. The workspace links the dynamic C runtime (/MD):
# the prebuilt ONNX Runtime (ort-sys) exists only as /MD, and rust-lld refuses to mix
# the two (/failifmismatch 'RuntimeLibrary'). A normal variable set here shadows that
# cache entry, so libaom is compiled with /MD like everything else.
set(CMAKE_MSVC_RUNTIME_LIBRARY "MultiThreadedDLL")
