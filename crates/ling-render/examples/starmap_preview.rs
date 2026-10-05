//! 成长星图渲染预览：用 ling-core 领域数据构建快照 → 推进若干帧 → 落盘 BMP。
//!
//! 运行：`cargo run -p ling-render --example starmap_preview -- [宽度 高度 帧数 输出路径]`
//! 产物为 24 位无压缩 BMP（不依赖任何图像库），可直接用系统看图工具打开，
//! 也演示了上层 UI 拿到 RGBA8 缓冲后如何进一步使用。

use ling_core::character::{Character, Identity};
use ling_core::memory::{MemoryArc, MemoryEvent};
use ling_render::{StarSnapshot, StarmapRenderer};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let width: u32 = args.get(1).map(|s| s.parse().unwrap()).unwrap_or(1280);
    let height: u32 = args.get(2).map(|s| s.parse().unwrap()).unwrap_or(720);
    let frames: usize = args.get(3).map(|s| s.parse().unwrap()).unwrap_or(480);
    let output = args.get(4).cloned().unwrap_or_else(|| "starmap.bmp".into());

    // —— 组装一份带特质与三层记忆的示例角色数据（ling-core 领域侧）——
    let mut character = Character::new(
        1,
        Identity::new("阿澈", "观星者", "北境雪原", "游侠"),
        1_700_000_000,
    );
    character.emerge_trait("沉静", "寡言但观察敏锐", 1_700_000_000);
    character.emerge_trait("执拗", "认定的事九头牛拉不回", 1_700_010_000);
    character.emerge_trait("共情", "对弱者的痛苦感同身受", 1_700_020_000);

    let events: Vec<MemoryEvent> = (0..6)
        .map(|i| MemoryEvent {
            id: 100 + i,
            summary: format!("事件 {i}"),
            keywords: vec!["北境".into(), "雪".into(), "篝火".into()],
            scene_ids: vec![i],
            start_time: 1_700_000_000 + i as i64 * 86_400,
            end_time: 1_700_000_000 + i as i64 * 86_400 + 3600,
        })
        .collect();
    let arcs = vec![MemoryArc {
        id: 200,
        title: "北行".into(),
        summary: "从流亡到归属".into(),
        keywords: vec!["北境".into()],
        event_ids: vec![100, 101, 102, 103],
        start_time: 1_700_000_000,
        end_time: 1_700_086_400,
    }];

    // —— 渲染：装载快照，推进布局，读回像素 ——
    let mut renderer = StarmapRenderer::new(width, height).expect("需要可用的 GPU 适配器");
    renderer.load_snapshot(StarSnapshot::from_character_memory(
        &character, &events, &arcs,
    ));
    println!(
        "星图：{} 节点 / {} 边；视口 {}x{}；推进 {frames} 帧",
        renderer.node_count(),
        renderer.edge_count(),
        renderer.width(),
        renderer.height()
    );
    let mut last_disp = 0.0;
    for i in 0..frames {
        last_disp = renderer.step_and_draw();
        if i % 60 == 0 {
            println!("  第 {i} 帧：单步最大位移 {last_disp:.3}");
        }
    }
    println!("布局末态单步最大位移：{last_disp:.3}");

    let rgba = renderer.read_pixels_rgba8().expect("读回失败");
    write_bmp24(&output, width, height, &rgba);
    println!("已写出：{output}（{} 字节 RGBA → BMP）", rgba.len());
}

/// 把自上而下、行优先的 RGBA8 像素写成 24 位 BMP（BGR 行序、4 字节行对齐）。
fn write_bmp24(path: &str, width: u32, height: u32, rgba: &[u8]) {
    assert_eq!(
        rgba.len(),
        (width as usize) * (height as usize) * 4,
        "像素长度不符"
    );
    let row = (width as usize * 3 + 3) & !3; // BMP 行按 4 字节对齐
    let image_size = row * height as usize;
    let file_size = 54 + image_size;
    let mut out = Vec::with_capacity(file_size);

    // BITMAPFILEHEADER（14 字节）
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&(file_size as u32).to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&54u32.to_le_bytes());
    // BITMAPINFOHEADER（40 字节）
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(width as i32).to_le_bytes());
    out.extend_from_slice(&(height as i32).to_le_bytes()); // 正值 = 自下而上
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&24u16.to_le_bytes());
    out.extend_from_slice(&[0; 24]);

    // 像素：BMP 自下而上、BGR；我们的缓冲自上而下、RGBA
    for y in (0..height as usize).rev() {
        let mut bgr_row = Vec::with_capacity(row);
        for x in 0..width as usize {
            let px = (y * width as usize + x) * 4;
            bgr_row.push(rgba[px + 2]); // B
            bgr_row.push(rgba[px + 1]); // G
            bgr_row.push(rgba[px]); // R
        }
        bgr_row.resize(row, 0);
        out.extend_from_slice(&bgr_row);
    }
    std::fs::write(path, out).expect("写 BMP 失败");
}
