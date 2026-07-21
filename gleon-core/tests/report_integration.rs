#![cfg(not(miri))]

use gleon_core::config::{DiffConfig, Mode};
use gleon_core::engine::{ComparisonResult, MismatchDetail, compare_images};
use gleon_core::report::ReportGenerator;
use gleon_core::scanner::{TestCaseResult, TestImageResult};
use std::fs;
use std::path::{Path, PathBuf};

#[test]
fn test_report_generation_with_real_images_and_durability() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let fixtures_dir = manifest_dir.join("tests").join("fixtures");

    let baseline_path = fixtures_dir.join("dashboard_baseline.png");
    let actual_path = fixtures_dir.join("dashboard_actual.png");

    let baseline_img = image::open(&baseline_path)
        .expect("Failed to open dashboard_baseline.png")
        .to_rgba8();
    let actual_img = image::open(&actual_path)
        .expect("Failed to open dashboard_actual.png")
        .to_rgba8();

    // 1. Выполняем сравнение для Pixel mode
    let diff_config = DiffConfig {
        threshold: 0.0,
        ..Default::default()
    };
    let comp_pixel_res = compare_images(&baseline_img, &actual_img, Mode::Pixel, &diff_config);
    let (pixel_detail, pixel_diff_img) = match comp_pixel_res {
        ComparisonResult::Mismatch { detail, diff_image } => (detail, diff_image),
        other => panic!("Expected ComparisonResult::Mismatch, got {:?}", other),
    };

    // 2. Выполняем сравнение для SSIM mode
    let ssim_config = DiffConfig {
        threshold: 1.0,
        ..Default::default()
    };
    let comp_ssim_res = compare_images(&baseline_img, &actual_img, Mode::Ssim, &ssim_config);
    let (ssim_detail, ssim_diff_img) = match comp_ssim_res {
        ComparisonResult::Mismatch { detail, diff_image } => (detail, diff_image),
        other => panic!("Expected ComparisonResult::Mismatch, got {:?}", other),
    };

    // 3. Создаем директорию фикстур для отчетов
    let report_dir = fixtures_dir.join("report_output");
    fs::create_dir_all(&report_dir).expect("Failed to create report output dir");

    let diff_pixel_name = "diff_dashboard_pixel.png";
    let diff_ssim_name = "diff_dashboard_ssim.png";

    pixel_diff_img
        .save(report_dir.join(diff_pixel_name))
        .expect("Failed to save pixel diff image");
    ssim_diff_img
        .save(report_dir.join(diff_ssim_name))
        .expect("Failed to save ssim diff image");

    // 4. Формируем комплексный TestCaseResult с всеми 4 типами ошибок
    let tc_res = TestCaseResult {
        name: "billing_dashboard".to_string(),
        results: vec![
            TestImageResult::Mismatch {
                relative_path: PathBuf::from("overview_metrics.png"),
                detail: pixel_detail,
                diff_path: PathBuf::from(diff_pixel_name),
                baseline_path: PathBuf::from("../dashboard_baseline.png"),
                actual_path: PathBuf::from("../dashboard_actual.png"),
            },
            TestImageResult::Mismatch {
                relative_path: PathBuf::from("revenue_performance.png"),
                detail: ssim_detail,
                diff_path: PathBuf::from(diff_ssim_name),
                baseline_path: PathBuf::from("../dashboard_baseline.png"),
                actual_path: PathBuf::from("../dashboard_actual.png"),
            },
            TestImageResult::Mismatch {
                relative_path: PathBuf::from("security_alert_banner.png"),
                detail: MismatchDetail::SsimFallback { diff_count: 14205 },
                diff_path: PathBuf::from(diff_pixel_name),
                baseline_path: PathBuf::from("../dashboard_baseline.png"),
                actual_path: PathBuf::from("../dashboard_actual.png"),
            },
            TestImageResult::DimensionMismatch {
                relative_path: PathBuf::from("sidebar_navigation.png"),
                baseline_size: (1920, 1080),
                actual_size: (1920, 1200),
                baseline_path: PathBuf::from("../dashboard_baseline.png"),
                actual_path: PathBuf::from("../dashboard_actual.png"),
            },
            TestImageResult::DecodeError {
                relative_path: PathBuf::from("user_avatar.png"),
                error: "PNG header corrupted or incomplete".to_string(),
            },
        ],
    };

    // 5. Генерируем HTML отчет
    let html_report =
        ReportGenerator::generate_html(std::slice::from_ref(&tc_res), Some(&report_dir));

    let html = html_report
        .expect("HTML render should succeed")
        .expect("Expected Some(HTML), but got None");
    assert!(!html.contains("data:image/png;base64,"));
    assert!(html.contains("billing_dashboard / overview_metrics.png"));
    assert!(html.contains("billing_dashboard / revenue_performance.png"));
    assert!(html.contains("billing_dashboard / sidebar_navigation.png"));
    assert!(html.contains("billing_dashboard / user_avatar.png"));
    assert!(html.contains("PNG header corrupted or incomplete"));

    let html_path = report_dir.join("report.html");
    fs::write(&html_path, &html).expect("Failed to write HTML report");
    assert!(html_path.exists());

    // 6. Генерируем и записываем JUnit XML
    let xml = ReportGenerator::generate_junit_xml(std::slice::from_ref(&tc_res))
        .expect("XML render should succeed");
    assert!(xml.contains("<testsuites name=\"Gleon Tests\""));
    assert!(xml.contains("<failure message=\"Visual mismatch detected ("));
    assert!(xml.contains("<failure message=\"Visual mismatch detected (SSIM score:"));
    assert!(xml.contains(
        "<failure message=\"Dimension mismatch (Baseline: 1920x1080, Actual: 1920x1200)\""
    ));
    assert!(xml.contains("<failure message=\"Decode error: PNG header corrupted or incomplete\""));

    let xml_path = report_dir.join("junit.xml");
    fs::write(&xml_path, &xml).expect("Failed to write XML report");
    assert!(xml_path.exists());

    // 7. Генерируем и записываем Markdown
    let md = ReportGenerator::generate_markdown(std::slice::from_ref(&tc_res));
    assert!(md.contains("## Gleon Visual Regression Summary"));
    assert!(md.contains("❌ Mismatch"));
    assert!(md.contains("❌ Dimension Mismatch"));
    assert!(md.contains("❌ Decode Error"));

    let md_path = report_dir.join("report.md");
    fs::write(&md_path, &md).expect("Failed to write MD report");
    assert!(md_path.exists());
}
