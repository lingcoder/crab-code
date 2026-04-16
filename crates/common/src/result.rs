pub type Result<T> = std::result::Result<T, crate::Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn result_ok() {
        let r: Result<i32> = Ok(42);
        if let Ok(v) = r {
            assert_eq!(v, 42);
        } else {
            panic!("expected Ok");
        }
    }

    #[test]
    fn result_err() {
        let r: Result<i32> = Err(crate::Error::Other("fail".into()));
        assert!(r.is_err());
    }
}
