#[cfg(windows)]
use std::env;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=vivida.rc");
        println!("cargo:rerun-if-changed=vivido.ico");
        embed_resource::compile("vivida.rc", embed_resource::NONE)
            .manifest_required()
            .expect("failed to embed the Vivida Windows icon");
        configure_ffmpeg_delay_load();
    }
}

#[cfg(windows)]
fn configure_ffmpeg_delay_load() {
    let runtime_dlls = env::var("DEP_VIVIDO_RUNTIME_FFMPEG_DELAY_LOAD")
        .expect("Vivido did not report its FFmpeg runtime DLLs");

    println!("cargo:rustc-link-arg-bin=vivida=/IGNORE:4199");
    println!("cargo:rustc-link-arg-bin=vivida=delayimp.lib");
    for dll in runtime_dlls.split(',').filter(|dll| !dll.is_empty()) {
        println!("cargo:rustc-link-arg-bin=vivida=/DELAYLOAD:{dll}");
    }
}
