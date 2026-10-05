//! ling-app 构建脚本：把工作区根目录 `ui/*.slint` 编译为 Rust 代码。
//!
//! - 入口 `ui/app.slint`（其中 `import` 逐级引用 `ui/theme.slint`）；
//! - 样式固定为 fluent-dark：标准控件（LineEdit / TextEdit 等）自动匹配深色单色主题。

fn main() {
    let config = slint_build::CompilerConfiguration::new()
        .with_style("fluent-dark".to_string());
    slint_build::compile_with_config("../../ui/app.slint", config).expect("编译 ui/app.slint 失败");
}
