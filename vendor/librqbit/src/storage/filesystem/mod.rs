mod fs;
mod opened_file;
mod sparse;

#[cfg(windows)]
pub use sparse::mark_file_sparse;
pub use fs::{FilesystemStorage, FilesystemStorageFactory};
#[cfg(test)]
pub(crate) use opened_file::OpenedFile;
pub use opened_file::OurFileExt;
