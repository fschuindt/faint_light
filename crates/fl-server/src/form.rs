//! Multipart request parsing, shared by both APIs.

use axum::extract::Multipart;
use axum::http::StatusCode;
use axum::response::Response;

use crate::solve::err_json;

/// A parsed multipart body: the uploaded file plus every text field, in
/// arrival order. A multipart body can only be read once, so endpoints that
/// need more than `file` collect everything here and pick afterwards.
pub struct Form {
    pub file: Option<(String, Vec<u8>)>,
    pub fields: Vec<(String, String)>,
}

impl Form {
    pub async fn read(mut mp: Multipart) -> Result<Form, Response> {
        let mut form = Form {
            file: None,
            fields: Vec::new(),
        };
        loop {
            let field = match mp.next_field().await {
                Ok(Some(f)) => f,
                Ok(None) => break,
                Err(e) => {
                    return Err(err_json(
                        StatusCode::BAD_REQUEST,
                        &format!("multipart: {e}"),
                    ))
                }
            };
            let name = field.name().unwrap_or("").to_string();
            if name == "file" {
                let filename = field.file_name().unwrap_or("upload").to_string();
                match field.bytes().await {
                    Ok(b) => form.file = Some((filename, b.to_vec())),
                    Err(e) => {
                        return Err(err_json(
                            StatusCode::BAD_REQUEST,
                            &format!("file read: {e}"),
                        ))
                    }
                }
            } else if let Ok(text) = field.text().await {
                form.fields.push((name, text));
            }
        }
        Ok(form)
    }

    pub fn field(&self, name: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    pub fn number(&self, name: &str) -> Option<f64> {
        self.field(name).and_then(|v| v.trim().parse().ok())
    }

    /// A checkbox-style flag. Absent is false; present without a value (as
    /// an HTML form posts a checked box) is true.
    pub fn flag(&self, name: &str) -> bool {
        match self.field(name).map(str::trim) {
            None => false,
            Some("") => true,
            Some(v) => matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"),
        }
    }

    /// Take the uploaded image, or the 400 to return without one.
    pub fn take_file(&mut self) -> Result<(String, Vec<u8>), Response> {
        match self.file.take() {
            None => Err(err_json(StatusCode::BAD_REQUEST, "no file field in upload")),
            Some((_, b)) if b.is_empty() => Err(err_json(StatusCode::BAD_REQUEST, "empty upload")),
            Some((name, b)) => Ok((name, b)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form(fields: &[(&str, &str)]) -> Form {
        Form {
            file: None,
            fields: fields
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    #[test]
    fn flags_accept_the_usual_spellings() {
        let f = form(&[
            ("a", "1"),
            ("b", "true"),
            ("c", "ON"),
            ("d", ""),
            ("e", "0"),
            ("f", "no"),
        ]);
        for name in ["a", "b", "c", "d"] {
            assert!(f.flag(name), "{name} should be true");
        }
        for name in ["e", "f", "missing"] {
            assert!(!f.flag(name), "{name} should be false");
        }
    }

    #[test]
    fn numbers_tolerate_surrounding_space() {
        let f = form(&[("lat", " 40.7128 "), ("bad", "north")]);
        assert_eq!(f.number("lat"), Some(40.7128));
        assert_eq!(f.number("bad"), None);
        assert_eq!(f.number("missing"), None);
    }
}
