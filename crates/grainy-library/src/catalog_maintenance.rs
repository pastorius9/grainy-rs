//! Port of `luma/catalog_maintenance.py`: resumable profile deduplication with a consistent backup and bounded
//! batches. Cancelling between steps leaves the catalogue as it was (a batch is one transaction); the safety
//! backup is made first and never replaces an earlier one.

use crate::profiles::{self, json_cell, pack_internal, payload_columns, verify_references};
use crate::{Cancel, Error, Result};
use grainy_catalog::json::equivalent;
use grainy_catalog::rusqlite::backup::{Backup, StepResult};
use grainy_catalog::rusqlite::{params, types::Value as Sql, Connection};
use std::path::Path;
use std::time::Duration;

/// What `optimize` reports (the oracle's `report` dict).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Profile payloads rewritten to references.
    pub converted: u64,
    /// Rows looked at.
    pub examined: u64,
    /// Rows to look at.
    pub total: u64,
    /// Unused assets deleted.
    pub removed_assets: u64,
    /// Assets left in `profile_assets`.
    pub profiles: u64,
    /// The run was cancelled.
    pub cancelled: bool,
    /// `VACUUM` ran.
    pub vacuumed: bool,
    /// At most 20 warnings, `"<owner>: <error>"`.
    pub warnings: Vec<String>,
    /// How many warnings there were (the key is absent from the oracle's report when 0).
    pub warning_count: u64,
    /// The safety backup file.
    pub backup: Option<String>,
    /// Catalogue size before, in bytes.
    pub before_bytes: u64,
    /// Catalogue size after, in bytes.
    pub after_bytes: u64,
    /// `max(0, before - after)`.
    pub saved_bytes: u64,
    /// `PRAGMA integrity_check` (`ok`).
    pub integrity: String,
    /// `PRAGMA wal_checkpoint(TRUNCATE)`: busy, log frames, checkpointed frames.
    pub checkpoint: Vec<i64>,
}

/// What the progress callback receives (`dict(stage, examined, total)`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Progress {
    /// The stage, worded for the person as in the oracle.
    pub stage: String,
    /// Rows looked at so far.
    pub examined: u64,
    /// Rows to look at.
    pub total: u64,
}

fn size(db: &Connection) -> Result<u64> {
    let pages: i64 = db.query_row("PRAGMA page_count", [], |r| r.get(0))?;
    let page: i64 = db.query_row("PRAGMA page_size", [], |r| r.get(0))?;
    Ok((pages * page).max(0) as u64)
}

fn random_hex8() -> String {
    use std::hash::{BuildHasher, Hasher};
    let value = std::collections::hash_map::RandomState::new().build_hasher().finish();
    format!("{value:016x}")[..8].to_owned()
}

/// Port of `catalog_maintenance.optimize` with the default batch size of 16 rows.
pub fn optimize(directory: &Path, cancel: Option<&Cancel>, progress: &mut dyn FnMut(Progress)) -> Result<Report> {
    optimize_with(directory, cancel, progress, 16)
}

/// Port of `catalog_maintenance.optimize(directory, cancel, progress, batch_size=...)`.
pub fn optimize_with(
    directory: &Path,
    cancel: Option<&Cancel>,
    progress: &mut dyn FnMut(Progress),
    batch_size: usize,
) -> Result<Report> {
    let cancel = cancel.cloned().unwrap_or_default();
    let mut report = Report::default();
    let db = Connection::open(directory.join("catalog.sqlite"))?;
    db.busy_timeout(Duration::from_secs(1))?;
    report.before_bytes = size(&db)?;
    let outcome = run(&db, directory, &cancel, progress, batch_size, &mut report);
    let _ = db.execute_batch("ROLLBACK");
    let _ = db.progress_handler(0, None::<fn() -> bool>);
    match outcome {
        Ok(()) => {}
        Err(Error::Cancelled) => report.cancelled = true,
        Err(other) => return Err(other),
    }
    let publish = |stage: &str, report: &Report, progress: &mut dyn FnMut(Progress)| {
        progress(Progress { stage: stage.into(), examined: report.examined, total: report.total });
    };
    publish("무결성 확인 중", &report, progress);
    report.integrity = db.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
    report.after_bytes = size(&db)?;
    report.saved_bytes = report.before_bytes.saturating_sub(report.after_bytes);
    let has_assets: bool =
        db.query_row("SELECT 1 FROM sqlite_master WHERE name='profile_assets'", [], |_| Ok(true)).unwrap_or(false);
    if has_assets {
        report.profiles = db.query_row("SELECT COUNT(*) FROM profile_assets", [], |r| r.get::<_, i64>(0))? as u64;
    }
    report.checkpoint = db.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| {
        Ok(vec![r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?])
    })?;
    Ok(report)
}

fn warning(report: &mut Report, owner: &str, error: &Error) {
    // Bounded diagnostic output even for a badly damaged large catalogue.
    report.warning_count += 1;
    if report.warnings.len() < 20 {
        report.warnings.push(format!("{owner}: {error}"));
    }
}

fn run(
    db: &Connection,
    directory: &Path,
    cancel: &Cancel,
    progress: &mut dyn FnMut(Progress),
    batch_size: usize,
    report: &mut Report,
) -> Result<()> {
    let check = || -> Result<()> {
        if cancel.is_set() {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    };
    macro_rules! publish {
        ($stage:expr) => {
            progress(Progress { stage: $stage.into(), examined: report.examined, total: report.total })
        };
    }
    check()?;
    publish!("안전 백업 중");
    let backup_dir = directory.join("backups");
    std::fs::create_dir_all(&backup_dir)?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let target = backup_dir.join(format!("before-optimize-{stamp}-{}.sqlite", random_hex8()));
    let partial = target.with_extension("sqlite.partial");
    let copied = (|| -> Result<()> {
        let mut output = Connection::open(&partial)?;
        {
            let backup = Backup::new(db, &mut output)?;
            loop {
                let step = backup.step(128)?;
                check()?;
                match step {
                    StepResult::Done => break,
                    StepResult::More => {}
                    _ => std::thread::sleep(Duration::from_millis(50)),
                }
            }
        }
        drop(output);
        check()?;
        std::fs::rename(&partial, &target)?;
        Ok(())
    })();
    // Only this job's incomplete backup; never touch prior backups.
    let _ = std::fs::remove_file(&partial);
    copied?;
    report.backup = Some(crate::pathx::text(&target));
    db.execute_batch("BEGIN")?;
    profiles::migrate(db)?;
    db.execute_batch("COMMIT")?;
    let columns = payload_columns(db, false)?;
    let mut total = 0u64;
    for (table, _, filter) in &columns {
        total += db.query_row(&format!("SELECT COUNT(*) FROM {table}{filter}"), [], |r| r.get::<_, i64>(0))? as u64;
    }
    report.total = total;
    let preview: bool =
        db.query_row("SELECT 1 FROM sqlite_master WHERE name='preview_state'", [], |_| Ok(true)).unwrap_or(false);
    for (table, column, filter) in &columns {
        let mut last: Option<i64> = None;
        loop {
            check()?;
            // Parenthesize the preference predicate before adding rowid.
            let base = if filter.is_empty() { String::new() } else { format!(" WHERE ({})", &filter[7..]) };
            let condition = match last {
                Some(_) => format!("{base}{}rowid>?", if base.is_empty() { " WHERE " } else { " AND " }),
                None => base,
            };
            let query = format!("SELECT rowid AS rid FROM {table}{condition} ORDER BY rowid LIMIT ?");
            let rows: Vec<i64> = {
                let mut stmt = db.prepare(&query)?;
                let bound: Vec<i64> = match last {
                    Some(l) => vec![l, batch_size as i64],
                    None => vec![batch_size as i64],
                };
                let mapped = stmt.query_map(grainy_catalog::rusqlite::params_from_iter(bound), |r| r.get(0))?;
                mapped.collect::<std::result::Result<_, _>>()?
            };
            if rows.is_empty() {
                break;
            }
            let mut converted = 0u64;
            let batch = (|| -> Result<()> {
                db.execute_batch("BEGIN IMMEDIATE")?;
                for rid in &rows {
                    check()?;
                    // Another writer may have changed this row after SELECT.
                    let current: Option<Sql> = db
                        .query_row(&format!("SELECT {column} FROM {table} WHERE rowid=?"), [rid], |r| r.get(0))
                        .map(Some)
                        .or_else(|e| match e {
                            grainy_catalog::rusqlite::Error::QueryReturnedNoRows => Ok(None),
                            other => Err(other),
                        })?;
                    let Some(current) = current else { continue };
                    let step = (|| -> Result<()> {
                        let value = json_cell(current)?;
                        db.execute_batch("SAVEPOINT profile_row")?;
                        let packed = pack_internal(db, &value)?;
                        if !equivalent(&packed, &value) {
                            let token: Option<String> = if *table == "photos" && preview {
                                db.query_row("SELECT token FROM preview_state WHERE photo_id=?", [rid], |r| r.get(0))
                                    .ok()
                            } else {
                                None
                            };
                            db.execute(
                                &format!("UPDATE {table} SET {column}=? WHERE rowid=?"),
                                params![grainy_catalog::json::dumps(&packed, true, false, false), rid],
                            )?;
                            if let Some(token) = token {
                                db.execute("UPDATE preview_state SET token=? WHERE photo_id=?", params![token, rid])?;
                            }
                            converted += 1;
                        }
                        db.execute_batch("RELEASE profile_row")?;
                        Ok(())
                    })();
                    match step {
                        Ok(()) => {}
                        Err(error @ (Error::Value(_) | Error::Type(_) | Error::Json(_))) => {
                            // JSON may have failed before the savepoint.
                            if db.execute_batch("ROLLBACK TO profile_row").is_ok() {
                                let _ = db.execute_batch("RELEASE profile_row");
                            }
                            warning(report, &format!("{table}/{rid}"), &error);
                        }
                        Err(other) => return Err(other),
                    }
                    report.examined += 1;
                }
                check()?;
                Ok(())
            })();
            match batch {
                Ok(()) => db.execute_batch("COMMIT")?,
                Err(error) => {
                    let _ = db.execute_batch("ROLLBACK");
                    return Err(error);
                }
            }
            report.converted += converted;
            last = rows.last().copied();
            publish!("중복 프로파일 정리 중");
        }
    }
    check()?;
    publish!("프로파일 연결 검사 중");
    let mut removed = 0u64;
    db.execute_batch("BEGIN IMMEDIATE")?;
    let sweep = (|| -> Result<()> {
        match verify_references(db, &mut || check()) {
            Ok(used) => {
                // Unreadable rows can conceal references; never collect then.
                if report.warning_count == 0 {
                    let shas: Vec<String> = {
                        let mut stmt = db.prepare("SELECT sha256 FROM profile_assets")?;
                        let rows = stmt.query_map([], |r| r.get(0))?.collect::<std::result::Result<_, _>>()?;
                        rows
                    };
                    for sha in shas {
                        check()?;
                        if !used.contains(&sha) {
                            db.execute("DELETE FROM profile_assets WHERE sha256=?", [&sha])?;
                            removed += 1;
                        }
                    }
                }
                Ok(())
            }
            Err(error @ (Error::Value(_) | Error::Type(_) | Error::Json(_))) => {
                warning(report, "프로파일 연결 검사", &error);
                Ok(())
            }
            Err(other) => Err(other),
        }
    })();
    match sweep {
        Ok(()) => db.execute_batch("COMMIT")?,
        Err(error) => {
            let _ = db.execute_batch("ROLLBACK");
            return Err(error);
        }
    }
    report.removed_assets = removed;
    check()?;
    publish!("빈 공간 회수 중");
    let flag = cancel.clone();
    db.progress_handler(1000, Some(move || flag.is_set()))?;
    match db.execute_batch("VACUUM") {
        Ok(()) => report.vacuumed = true,
        Err(error) => {
            if cancel.is_set() {
                return Err(Error::Cancelled);
            }
            warning(report, "빈 공간 회수", &Error::Sql(error));
        }
    }
    let _ = db.progress_handler(0, None::<fn() -> bool>);
    check()?;
    Ok(())
}
