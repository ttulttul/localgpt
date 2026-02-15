fn main() {
    // Compile glibc compat shim for systems with glibc < 2.38.
    // The pre-built ONNX Runtime (via ort-sys) references __isoc23_strtol*
    // symbols that were introduced in glibc 2.38.
    #[cfg(target_os = "linux")]
    {
        cc::Build::new()
            .file("compat/glibc_compat.c")
            .compile("glibc_compat");
    }
}
