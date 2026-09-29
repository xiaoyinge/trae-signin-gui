fn main() {
    // 图标文件变化必须触发 build script 重跑：tauri_build 在此阶段把 icons/icon.ico
    // 嵌入 exe 资源；不声明 rerun-if-changed 时改图标只会得到旧图标（2026-09-29 踩坑）
    println!("cargo:rerun-if-changed=icons");
    tauri_build::build()
}
