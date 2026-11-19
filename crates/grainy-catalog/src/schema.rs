use crate::{Catalog, Result};
use rusqlite::OptionalExtension;
pub(crate) fn exists(c: &Catalog, name: &str) -> Result<bool> {
    Ok(c.db.query_row("SELECT 1 FROM sqlite_master WHERE name=?", [name], |r| r.get::<_, i64>(0)).optional()?.is_some())
}
pub(crate) fn migrate(c: &Catalog) -> Result<()> {
    c.db.execute_batch(include_str!("../assets/catalog.__init__.sql"))?;
    let columns = c.rows("PRAGMA table_info(photos)", [])?;
    if !columns.iter().any(|r| r["name"] == "virtual_source") {
        if c.count_photos()? > 0 {
            c.backup(c.directory.join("backups/before-library-v2.sqlite"))?;
        }
        c.transaction(||{
            c.db.execute_batch(include_str!("../assets/catalog._migrate_photos.sql"))?;
            c.db.execute_batch("INSERT INTO photos_v2(id,path,name,rating,flag,keywords,edits,metadata,imported) SELECT id,path,name,rating,flag,keywords,edits,metadata,imported FROM photos; DROP TABLE photos; ALTER TABLE photos_v2 RENAME TO photos; CREATE INDEX photos_path ON photos(path COLLATE NOCASE); CREATE INDEX photos_stack ON photos(stack_id);")?;Ok(())
        })?;
    }
    let browser = exists(c, "browser_index")?;
    if !browser && !c.directory.join("backups/before-browser-v3.sqlite").exists() && c.count_photos()? > 0 {
        c.backup(c.directory.join("backups/before-browser-v3.sqlite"))?;
    }
    c.db.execute_batch(include_str!("../assets/catalog._migrate_browser.sql"))?;
    if !browser {
        c.db.execute("INSERT OR IGNORE INTO browser_dirty SELECT id FROM photos", [])?;
    }
    c.db.execute_batch(include_str!("../assets/preview_store.migrate.sql"))?;
    if !exists(c, "profile_assets")?
        && !c.directory.join("backups/before-profile-assets-v1.sqlite").exists()
        && c.count_photos()? > 0
    {
        c.backup(c.directory.join("backups/before-profile-assets-v1.sqlite"))?;
    }
    c.db.execute_batch(include_str!("../assets/profile_store.migrate.sql"))?;
    c.db.execute_batch(include_str!("../assets/dust_store.migrate.sql"))?;
    let mut roots = c.folder_roots()?;
    if roots.is_empty() {
        for p in c.photos()? {
            let parent = c.platform.parent(crate::string(&p["path"])?);
            if !roots.iter().any(|r| c.platform.contains_folder(r, &parent)) {
                c.db.execute("INSERT OR IGNORE INTO folder_roots VALUES(?)", [&parent])?;
                roots.push(parent);
            }
        }
    }
    Ok(())
}
