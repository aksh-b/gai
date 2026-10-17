/// A new sortable, prefixed identifier such as `run_01j9z3k...`.
///
/// ULIDs sort by creation time, which keeps runs, steps and events in order
/// in storage without a separate sequence column.
pub fn new_id(prefix: &str) -> String {
    format!("{prefix}_{}", ulid::Ulid::new().to_string().to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_prefixed_and_sortable() {
        let a = new_id("run");
        std::thread::sleep(std::time::Duration::from_millis(2));
        let b = new_id("run");
        assert!(a.starts_with("run_"));
        assert!(a < b);
    }
}
