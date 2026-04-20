use std::sync::Mutex;

pub struct Clipboard {
    inner: Mutex<Option<arboard::Clipboard>>,
}

impl Clipboard {
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(arboard::Clipboard::new().ok()),
        }
    }

    pub fn copy(&self, text: &str) -> Result<(), String> {
        let mut guard = self.inner.lock().map_err(|e| e.to_string())?;
        match guard.as_mut() {
            Some(cb) => cb
                .set_text(text.to_string())
                .map_err(|e| e.to_string()),
            None => Err("clipboard unavailable".into()),
        }
    }
}

impl Default for Clipboard {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_does_not_panic() {
        let _cb = Clipboard::new();
    }

    #[test]
    fn copy_returns_result() {
        let cb = Clipboard::new();
        let _ = cb.copy("test");
    }
}
