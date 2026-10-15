//! Custom theme colours, fonts and themes; Save Current Theme and Browse for Themes (#1869).

use std::path::PathBuf;

use serde_json::{Value, json};
use wordcraft_doc::props::{Rgb, TextColor};

use crate::Session;

fn s() -> Session {
    Session::new(wordcraft_doc::Document::new())
}

fn run(s: &mut Session, id: &str, v: Value) -> Value {
    s.run(id, &v).unwrap_or_else(|e| panic!("{id}: {e}"))
}

/// Twelve distinct colours: dk1, lt1, dk2, lt2, accent1–6, hlink, folHlink.
fn twelve(seed: u8) -> Vec<String> {
    (0..12u8).map(|i| format!("#{:02X}{:02X}{:02X}", seed, i * 20, 0x80 + i)).collect()
}

fn rgb(h: &str) -> Rgb {
    Rgb::parse(h).unwrap()
}

/// A folder of its own for one test.
fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("wordcraft-themes-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn heading1_color(s: &Session) -> Option<TextColor> {
    s.doc.styles.styles.iter().find(|st| st.id == "Heading1").and_then(|st| st.chr.color)
}

#[test]
fn custom_colors_create_and_apply_all_twelve() {
    let mut s = s();
    let c = twelve(0x10);
    run(&mut s, "design.customColors", json!({"name": "Harbour", "colors": c}));
    let want: Vec<Rgb> = c.iter().map(|h| rgb(h)).collect();
    assert_eq!(s.doc.settings.theme_colors, want);
    assert_eq!(heading1_color(&s), Some(TextColor::Rgb(want[4])), "headings take accent 1");
    let list = run(&mut s, "design.customThemes", json!({}));
    assert_eq!(list["colors"][0]["name"], "Harbour");
    assert_eq!(list["colors"][0]["colors"].as_array().unwrap().len(), 12);
}

#[test]
fn custom_colors_name_taken_needs_replace() {
    let mut s = s();
    run(&mut s, "design.customColors", json!({"name": "Harbour", "colors": twelve(1)}));
    assert!(s.run("design.customColors", &json!({"name": "harbour", "colors": twelve(2)})).is_err(), "same name (any case) without replace");
    run(&mut s, "design.customColors", json!({"name": "Harbour", "colors": twelve(3), "replace": true}));
    let list = run(&mut s, "design.customThemes", json!({}));
    assert_eq!(list["colors"].as_array().unwrap().len(), 1);
    assert_eq!(list["colors"][0]["colors"][0], twelve(3)[0].trim_start_matches('#'));
}

#[test]
fn custom_fonts_create_and_apply() {
    let mut s = s();
    run(&mut s, "design.customFonts", json!({"name": "Bookish", "heading": "Palatino", "body": "Optima"}));
    assert_eq!(s.doc.settings.major_font, "Palatino");
    assert_eq!(s.doc.settings.minor_font, "Optima");
    assert_eq!(s.doc.styles.default_chr.font.as_deref(), Some("Optima"));
    let list = run(&mut s, "design.customThemes", json!({}));
    assert_eq!(list["fonts"][0], json!({"name": "Bookish", "heading": "Palatino", "body": "Optima"}));
}

#[test]
fn theme_colors_and_fonts_apply_custom_sets_by_name() {
    let mut s = s();
    let c = twelve(0x22);
    run(&mut s, "design.customColors", json!({"name": "Harbour", "colors": c}));
    run(&mut s, "design.customFonts", json!({"name": "Bookish", "heading": "Palatino", "body": "Optima"}));
    run(&mut s, "design.theme", json!({"name": "Studio"}));
    assert_ne!(s.doc.settings.theme_colors[4], rgb(&c[4]));
    run(&mut s, "design.themeColors", json!({"name": "Harbour"}));
    assert_eq!(s.doc.settings.theme_colors[0], rgb(&c[0]));
    assert_eq!(s.doc.settings.theme_colors[11], rgb(&c[11]));
    run(&mut s, "design.themeFonts", json!({"name": "Bookish"}));
    assert_eq!((s.doc.settings.major_font.as_str(), s.doc.settings.minor_font.as_str()), ("Palatino", "Optima"));
    // The old form still works.
    run(&mut s, "design.themeFonts", json!({"heading": "Georgia"}));
    assert_eq!(s.doc.settings.minor_font, "Georgia");
    assert!(s.run("design.themeFonts", &json!({"name": "Nope"})).is_err());
}

#[test]
fn delete_custom_removes_and_unknown_is_an_error() {
    let mut s = s();
    run(&mut s, "design.customColors", json!({"name": "Harbour", "colors": twelve(1)}));
    run(&mut s, "design.customFonts", json!({"name": "Bookish", "heading": "A", "body": "B"}));
    run(&mut s, "design.deleteCustom", json!({"kind": "colors", "name": "Harbour"}));
    run(&mut s, "design.deleteCustom", json!({"kind": "fonts", "name": "bookish"}));
    let list = run(&mut s, "design.customThemes", json!({}));
    assert!(list["colors"].as_array().unwrap().is_empty());
    assert!(list["fonts"].as_array().unwrap().is_empty());
    assert!(s.run("design.themeColors", &json!({"name": "Harbour"})).is_err());
    assert!(s.run("design.deleteCustom", &json!({"kind": "colors", "name": "Harbour"})).is_err());
    assert!(s.run("design.deleteCustom", &json!({"kind": "effects", "name": "x"})).is_err());
}

#[test]
fn custom_sets_persist_through_the_themes_folder() {
    let dir = temp_dir("persist");
    let mut a = s();
    a.load_custom_themes(&dir);
    run(&mut a, "design.customColors", json!({"name": "Harbour", "colors": twelve(5)}));
    run(&mut a, "design.customFonts", json!({"name": "Bookish", "heading": "Palatino", "body": "Optima"}));
    run(&mut a, "design.customFonts", json!({"name": "Gone", "heading": "X", "body": "Y"}));
    run(&mut a, "design.deleteCustom", json!({"kind": "fonts", "name": "Gone"}));
    run(&mut a, "design.themeFonts", json!({"name": "Bookish"}));
    run(&mut a, "design.saveTheme", json!({"name": "Seaside"}));
    assert!(dir.join("Seaside.thmx").is_file(), "Save Current Theme without a path goes to the themes folder");

    let mut b = s();
    b.load_custom_themes(&dir);
    let list = run(&mut b, "design.customThemes", json!({}));
    assert_eq!(list["colors"][0]["name"], "Harbour");
    assert_eq!(list["fonts"].as_array().unwrap().len(), 1);
    assert_eq!(list["fonts"][0]["name"], "Bookish");
    assert_eq!(list["themes"], json!(["Seaside"]));
    run(&mut b, "design.theme", json!({"name": "Seaside"}));
    assert_eq!(b.doc.settings.theme_colors[4], rgb(&twelve(5)[4]));
    assert_eq!(b.doc.settings.major_font, "Palatino");
    assert_eq!(b.doc.settings.theme_name, "Seaside");

    run(&mut b, "design.deleteCustom", json!({"kind": "theme", "name": "Seaside"}));
    assert!(!dir.join("Seaside.thmx").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn save_then_open_theme_restores_colours_fonts_and_name() {
    let dir = temp_dir("saveopen");
    let path = dir.join("Mine.thmx");
    let mut s = s();
    let c = twelve(0x33);
    run(&mut s, "design.customColors", json!({"name": "Harbour", "colors": c}));
    run(&mut s, "design.customFonts", json!({"name": "Bookish", "heading": "Palatino", "body": "Optima"}));
    s.doc.settings.theme_name = "Mine".into();
    run(&mut s, "design.saveTheme", json!({"path": path.to_string_lossy()}));
    assert!(path.is_file());

    let mut t = Session::new(wordcraft_doc::Document::new());
    run(&mut t, "design.openTheme", json!({"path": path.to_string_lossy()}));
    let want: Vec<Rgb> = c.iter().map(|h| rgb(h)).collect();
    assert_eq!(t.doc.settings.theme_colors, want);
    assert_eq!((t.doc.settings.major_font.as_str(), t.doc.settings.minor_font.as_str()), ("Palatino", "Optima"));
    assert_eq!(t.doc.settings.theme_name, "Mine");
    assert_eq!(heading1_color(&t), Some(TextColor::Rgb(want[4])));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn open_theme_reads_a_docx_and_base64_data() {
    let mut src = s();
    run(&mut src, "design.customColors", json!({"name": "Harbour", "colors": twelve(0x44)}));
    let bytes = wordcraft_docx::write(&src.doc).unwrap();
    let data = crate::cmd::insert::base64_encode(&bytes);
    let mut t = s();
    run(&mut t, "design.openTheme", json!({"path": "Other.docx", "data": data}));
    assert_eq!(t.doc.settings.theme_colors[7], rgb(&twelve(0x44)[7]));
    assert!(t.run("design.openTheme", &json!({"path": "x.thmx", "data": "bm90IGEgemlw"})).is_err(), "not a package");
}

#[test]
fn docx_saved_after_a_custom_theme_carries_it() {
    let mut s = s();
    let c = twelve(0x55);
    run(&mut s, "design.customColors", json!({"name": "Harbour", "colors": c}));
    run(&mut s, "design.customFonts", json!({"name": "Bookish", "heading": "Palatino", "body": "Optima"}));
    let bytes = wordcraft_docx::write(&s.doc).unwrap();
    let t = wordcraft_docx::read_theme(&bytes).unwrap();
    let want: Vec<Rgb> = c.iter().map(|h| rgb(h)).collect();
    assert_eq!(t.colors.to_vec(), want);
    assert_eq!((t.major_font.as_str(), t.minor_font.as_str()), ("Palatino", "Optima"));
}

#[test]
fn hostile_custom_theme_params_are_errors() {
    let mut s = s();
    let mut eleven = twelve(1);
    eleven.pop();
    let mut bad = twelve(1);
    bad[3] = "#GG0000".into();
    let cases = [
        ("design.customColors", json!({"name": "A", "colors": eleven})),
        ("design.customColors", json!({"name": "A", "colors": bad})),
        ("design.customColors", json!({"name": "A", "colors": [1, 2, 3]})),
        ("design.customColors", json!({"name": "", "colors": twelve(1)})),
        ("design.customColors", json!({"name": "   ", "colors": twelve(1)})),
        ("design.customColors", json!({"name": "../evil", "colors": twelve(1)})),
        ("design.customColors", json!({"name": "a/b", "colors": twelve(1)})),
        ("design.customColors", json!({"name": "a\\b", "colors": twelve(1)})),
        ("design.customColors", json!({"name": "x".repeat(10_000), "colors": twelve(1)})),
        ("design.customColors", json!({"name": "Craft", "colors": twelve(1)})),
        ("design.customFonts", json!({"name": "F", "heading": "", "body": "B"})),
        ("design.customFonts", json!({"name": "..", "heading": "A", "body": "B"})),
        ("design.customFonts", json!({"name": "F", "heading": "A".repeat(10_000), "body": "B"})),
        ("design.deleteCustom", json!({"kind": "colors"})),
        ("design.saveTheme", json!({"name": "x"})),
        ("design.saveTheme", json!({"name": "../../evil"})),
        ("design.openTheme", json!({"path": ""})),
        ("design.openTheme", json!({"path": "/no/such/file.thmx"})),
    ];
    for (id, v) in cases {
        assert!(s.run(id, &v).is_err(), "{id} {v} should be an error");
    }
    let list = run(&mut s, "design.customThemes", json!({}));
    assert!(list["colors"].as_array().unwrap().is_empty());
    assert!(list["fonts"].as_array().unwrap().is_empty());
}

#[test]
fn hostile_themes_folder_is_survived() {
    let dir = temp_dir("hostile");
    std::fs::write(dir.join(crate::cmd::themes::CUSTOM_FILE), b"{not json").unwrap();
    std::fs::write(dir.join("Broken.thmx"), b"PK\x03\x04garbage").unwrap();
    let mut s = s();
    s.load_custom_themes(&dir);
    let list = run(&mut s, "design.customThemes", json!({}));
    assert!(list["colors"].as_array().unwrap().is_empty());
    assert!(list["themes"].as_array().unwrap().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}
