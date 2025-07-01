#[macro_export]
macro_rules! handle_status_result {
    ($sender:expr, $res:expr) => {
        match $res {
            Ok(val) => val,
            Err(e) => {
                crate::status::handle_error(&$sender, e.into()).await;
                return Err(e.into());
            }
        }
    };
}
