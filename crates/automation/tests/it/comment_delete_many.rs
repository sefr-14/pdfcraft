//! This fork (COMMENTS.1): `comment_delete_many`, end to end: counts, one undo step, a reply only
//! with its comment, refused whole.

use std::path::{Path, PathBuf};

use pdfcraft_automation::{Automation, Content, ToolError};
use serde_json::{Value, json};

/// One page: comment R with replies r1 and r2 (r2 to r1), comment Q with reply q1, comment U.
fn threads() -> Vec<u8> {
    let text = |what: &str, extra: &str| format!("<< /Type /Annot /Subtype /Text /Rect [0 0 20 20] /Contents ({what}) /NM ({what}){extra} >>");
    let objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Annots [4 0 R 5 0 R 6 0 R 7 0 R 8 0 R 9 0 R] >>".into(),
        text("R", ""),
        text("r1", " /IRT 4 0 R"),
        text("r2", " /IRT 5 0 R"),
        text("Q", ""),
        text("q1", " /IRT 7 0 R"),
        text("U", ""),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let x = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

fn workdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pdfcraft-automation-comment-delete-many-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.pdf"), threads()).unwrap();
    dir
}

fn auto(dir: &Path) -> Automation {
    Automation::new().with_root(dir).unwrap().with_clock(|| 1_700_000_000)
}

fn ok(a: &mut Automation, tool: &str, args: Value) -> Value {
    match a.call(tool, &args) {
        Ok(mut c) => match c.remove(0) {
            Content::Json(v) => v,
            other => panic!("{tool}: expected JSON, got {other:?}"),
        },
        Err(e) => panic!("{tool} {args}: {e}"),
    }
}

/// The comments and replies left, by text, in list order.
fn left(a: &mut Automation, doc: u64) -> Vec<String> {
    let list = ok(a, "comment_list", json!({ "doc": doc }));
    let mut out = Vec::new();
    for c in list["comments"].as_array().into_iter().flatten() {
        out.push(c["contents"].as_str().unwrap_or_default().to_string());
        for r in c["replies"].as_array().into_iter().flatten() {
            out.push(r["contents"].as_str().unwrap_or_default().to_string());
        }
    }
    out
}

#[test]
fn several_comments_go_at_once_with_their_replies_counted_once() {
    let dir = workdir("counts");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    let out = ok(&mut a, "comment_delete_many", json!({ "doc": doc, "comments": [{ "id": "R" }, { "id": "r1" }, { "page": 1, "index": 4 }] }));
    assert_eq!((out["deleted_comments"].as_u64(), out["deleted_replies"].as_u64()), (Some(2), Some(3)));
    assert_eq!(left(&mut a, doc), ["U"]);
    ok(&mut a, "edit_undo", json!({ "doc": doc }));
    assert_eq!(left(&mut a, doc).len(), 6, "one undo step");
}

#[test]
fn a_reply_alone_or_a_bad_argument_deletes_nothing() {
    let dir = workdir("refused");
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    let before = left(&mut a, doc);
    assert!(matches!(a.call("comment_delete_many", &json!({ "doc": doc, "comments": [{ "id": "r1" }] })), Err(ToolError::Failed(_))));
    assert!(matches!(a.call("comment_delete_many", &json!({ "doc": doc, "comments": [{ "id": "Q" }, { "id": "r1" }] })), Err(ToolError::Failed(_))));
    assert!(a.call("comment_delete_many", &json!({ "doc": doc, "comments": [{ "id": "R" }, { "id": "nope" }] })).is_err());
    for bad in [json!([]), json!("R"), json!([1])] {
        assert!(matches!(a.call("comment_delete_many", &json!({ "doc": doc, "comments": bad })), Err(ToolError::InvalidArgs(_))), "{bad}");
    }
    assert_eq!(left(&mut a, doc), before, "nothing went");
}
