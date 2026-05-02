use crate::cache::SyncCache;
use crate::error::AppError;
use std::path::PathBuf;

pub(crate) fn rebuild_cache_command(input_dir: PathBuf) -> Result<(), AppError> {
    let mut cache = SyncCache::load(&input_dir)?;
    cache.rebuild_local_state(&input_dir);
    cache.save(&input_dir)?;

    println!("Rebuilt cache under {}.", input_dir.display());
    Ok(())
}
