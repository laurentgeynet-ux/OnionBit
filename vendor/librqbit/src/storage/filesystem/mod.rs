mod fs;
mod opened_file;
mod sparse;

pub use fs::{FilesystemStorage, FilesystemStorageFactory};
#[cfg(test)]
pub(crate) use opened_file::OpenedFile;
pub use opened_file::OurFileExt;
