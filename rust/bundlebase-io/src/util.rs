//! Generic URL and path utilities for IO operations.
//!
//! These utilities are protocol-agnostic and used across different storage backends.

use crate::BundlebaseError;
use object_store::path::Path;
use url::Url;

/// Like Url::join but allows an input with multiple sub-paths. The appended path is always treated as a relative path.
pub fn join_url(base: &Url, append: &str) -> Result<Url, BundlebaseError> {
    let mut return_url = if !base.path().ends_with('/') {
        Url::parse(format!("{}/", base).as_str())?
    } else {
        base.clone()
    };
    for segment in append.split("/").filter(|s| !s.is_empty()) {
        return_url = return_url.join(format!("{}/", segment).as_str())?;
    }
    if !append.ends_with('/') {
        return_url.set_path(return_url.path().trim_end_matches('/').to_string().as_str());
    }
    Ok(return_url)
}

pub fn join_path(base: &Path, append: &str) -> Result<Path, BundlebaseError> {
    let mut obj_path = base.clone();
    for segment in append.split('/').filter(|s| !s.is_empty()) {
        if segment == ".." {
            let mut path_str = obj_path.to_string();
            if path_str.ends_with('/') {
                path_str = path_str[0..path_str.len() - 1].to_string();
            }
            obj_path = Path::parse(match path_str.rfind("/") {
                Some(idx) => path_str[0..idx].to_string(),
                None => "/".to_string(),
            })?;
        } else {
            obj_path = obj_path.child(segment);
        }
    }
    if append.ends_with('/') {
        obj_path = obj_path.child("");
    }
    Ok(obj_path)
}

/// A stream wrapper that holds a guard object alive until the stream is dropped.
/// Used to keep temp files alive while streaming their contents.
pub(crate) struct GuardedStream<S> {
    inner: S,
    _guard: std::sync::Arc<tempfile::NamedTempFile>,
}

impl<S: futures::Stream + Unpin> futures::Stream for GuardedStream<S> {
    type Item = S::Item;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        std::pin::Pin::new(&mut self.inner).poll_next(cx)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

/// Stream bytes from a temp file in chunks using `ReaderStream`.
///
/// The `NamedTempFile` handle is held via `Arc` so the underlying file
/// is not deleted until the stream is fully consumed or dropped.
pub(crate) fn stream_from_temp_file(
    temp: tempfile::NamedTempFile,
) -> futures::stream::BoxStream<'static, Result<bytes::Bytes, std::io::Error>> {
    use std::sync::Arc;

    let std_file = match temp.reopen() {
        Ok(f) => f,
        Err(e) => {
            return Box::pin(futures::stream::once(async move { Err(e) }));
        }
    };
    let async_file = tokio::fs::File::from_std(std_file);
    let reader_stream = tokio_util::io::ReaderStream::new(async_file);

    let _guard = Arc::new(temp);
    Box::pin(GuardedStream {
        inner: reader_stream,
        _guard,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case(
        "s3://bucket/path/to/dir",
        "file.txt",
        "s3://bucket/path/to/dir/file.txt"
    )]
    #[case("memory:///path/to/dir", "file.txt", "memory:///path/to/dir/file.txt")]
    #[case("memory:///base", "dir", "memory:///base/dir")]
    #[case("memory:///base", "more/here", "memory:///base/more/here")]
    #[case("memory:///base/", "more/here", "memory:///base/more/here")]
    #[case("memory:///base/", "/more/here", "memory:///base/more/here")]
    #[case("memory:///base/", "/more/here/", "memory:///base/more/here/")]
    #[case("memory:///path/to/dir", "../file.txt", "memory:///path/to/file.txt")]
    #[case("memory:///path/to/dir", "../../file.txt", "memory:///path/file.txt")]
    fn test_join_url(#[case] base: &str, #[case] append: &str, #[case] expected: &str) {
        assert_eq!(
            expected,
            join_url(&Url::parse(base).unwrap(), append)
                .unwrap()
                .to_string()
        )
    }

    #[rstest]
    #[case("path/to/dir", "file.txt", "path/to/dir/file.txt")]
    #[case("/path/to/dir", "file.txt", "path/to/dir/file.txt")]
    #[case("/path/to/dir", "../file.txt", "path/to/file.txt")]
    #[case("/path/to/dir", "../../file.txt", "path/file.txt")]
    #[case("/path/to/dir", "../../../file.txt", "file.txt")]
    #[case("/path/to/dir", "../../../../file.txt", "file.txt")]
    #[case("/base", "dir", "base/dir")]
    #[case("/base", "more/here", "base/more/here")]
    #[case("/base/", "more/here", "base/more/here")]
    #[case("/base/", "/more/here", "base/more/here")]
    #[case("/base/", "/more/here/", "base/more/here/")]
    fn test_join_path(#[case] base: &str, #[case] append: &str, #[case] expected: &str) {
        assert_eq!(
            expected,
            join_path(&Path::parse(base).unwrap(), append)
                .unwrap()
                .to_string()
        )
    }
}
