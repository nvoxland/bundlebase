//! Directory IO traits.
//!
//! Trait definitions (`IOReadDir`, `IOReadWriteDir`, `WriteResult`) live in
//! `bundlebase_common::io_dir` and are re-exported here for convenience.

pub use bundlebase_common::io_dir::{IOReadDir, IOReadWriteDir, WriteResult};

#[cfg(test)]
mod tests {
    use crate::test_utils::random_memory_dir;

    #[test]
    fn test_relative_path_simple_file() {
        let dir = random_memory_dir();
        let file = dir.file("test.parquet").unwrap();

        let relative = dir.relative_path(file.as_ref()).unwrap();
        assert_eq!(relative, "test.parquet");
    }

    #[test]
    fn test_relative_path_nested_file() {
        let dir = random_memory_dir();
        let subdir = dir.subdir("ab").unwrap();
        let file = subdir.file("cdef12345678.parquet").unwrap();

        let relative = dir.relative_path(file.as_ref()).unwrap();
        assert_eq!(relative, "ab/cdef12345678.parquet");
    }

    #[test]
    fn test_relative_path_deeply_nested() {
        let dir = random_memory_dir();
        let sub1 = dir.subdir("level1").unwrap();
        let sub2 = sub1.subdir("level2").unwrap();
        let file = sub2.file("deep.json").unwrap();

        let relative = dir.relative_path(file.as_ref()).unwrap();
        assert_eq!(relative, "level1/level2/deep.json");
    }

    #[test]
    fn test_relative_path_file_not_in_directory() {
        let dir1 = random_memory_dir();
        let dir2 = random_memory_dir();
        let file = dir2.file("test.parquet").unwrap();

        let result = dir1.relative_path(file.as_ref());
        assert!(result.is_err());
    }
}
