//! Índice local em SQLite (modo WAL). Migrações por `PRAGMA user_version`.

use crate::Result;
use avs_core::{Id, Project, Time};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use ts_rs::TS;

const MIGRATIONS: &[&str] = &[
    // v1
    r#"
    CREATE TABLE projects (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        dir TEXT NOT NULL,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        duration INTEGER NOT NULL DEFAULT 0,
        width INTEGER NOT NULL,
        height INTEGER NOT NULL,
        fps TEXT NOT NULL,
        preset_id TEXT,
        status TEXT NOT NULL DEFAULT 'idle',
        thumbnail TEXT,
        asset_count INTEGER NOT NULL DEFAULT 0
    );
    CREATE TABLE jobs (
        id TEXT PRIMARY KEY,
        project_id TEXT,
        kind TEXT NOT NULL,
        title TEXT NOT NULL,
        state TEXT NOT NULL,
        stage TEXT,
        progress REAL NOT NULL DEFAULT 0,
        priority INTEGER NOT NULL,
        created_at TEXT NOT NULL,
        started_at TEXT,
        finished_at TEXT,
        error TEXT,
        payload TEXT NOT NULL,
        result TEXT
    );
    CREATE INDEX jobs_state ON jobs(state);
    CREATE TABLE exports (
        id TEXT PRIMARY KEY,
        project_id TEXT NOT NULL,
        path TEXT NOT NULL,
        created_at TEXT NOT NULL,
        duration INTEGER NOT NULL,
        width INTEGER NOT NULL,
        height INTEGER NOT NULL,
        codec TEXT NOT NULL,
        size_bytes INTEGER NOT NULL,
        report TEXT NOT NULL
    );
    CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
    "#,
];

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProjectSummary {
    pub id: Id,
    pub name: String,
    pub dir: PathBuf,
    pub created_at: String,
    pub updated_at: String,
    pub duration: Time,
    pub width: u32,
    pub height: u32,
    pub fps: String,
    pub preset_id: Option<String>,
    pub status: String,
    pub thumbnail: Option<PathBuf>,
    pub asset_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct JobRecord {
    pub id: Id,
    pub project_id: Option<Id>,
    pub kind: String,
    pub title: String,
    pub state: String,
    pub stage: Option<String>,
    pub progress: f32,
    pub priority: i32,
    pub created_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub error: Option<String>,
    #[ts(type = "unknown")]
    pub payload: serde_json::Value,
    #[ts(type = "unknown")]
    pub result: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ExportRecord {
    pub id: Id,
    pub project_id: Id,
    pub path: PathBuf,
    pub created_at: String,
    pub duration: Time,
    pub width: u32,
    pub height: u32,
    pub codec: String,
    #[ts(type = "number")]
    pub size_bytes: u64,
    #[ts(type = "unknown")]
    pub report: serde_json::Value,
}

pub struct Db {
    conn: Connection,
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let mut db = Db { conn };
        db.migrate()?;
        Ok(db)
    }

    pub fn open_in_memory() -> Result<Self> {
        let mut db = Db { conn: Connection::open_in_memory()? };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&mut self) -> Result<()> {
        let v: u32 = self.conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(v as usize) {
            let tx = self.conn.transaction()?;
            tx.execute_batch(sql)?;
            tx.pragma_update(None, "user_version", (i + 1) as u32)?;
            tx.commit()?;
        }
        Ok(())
    }

    pub fn journal_mode(&self) -> Result<String> {
        Ok(self.conn.pragma_query_value(None, "journal_mode", |r| r.get(0))?)
    }

    // ── projetos ──

    pub fn upsert_project(&self, p: &Project, dir: &Path, thumbnail: Option<&Path>, status: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO projects (id,name,dir,created_at,updated_at,duration,width,height,fps,preset_id,status,thumbnail,asset_count)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)
             ON CONFLICT(id) DO UPDATE SET name=?2, dir=?3, updated_at=?5, duration=?6, width=?7, height=?8, fps=?9,
               preset_id=?10, status=?11, thumbnail=COALESCE(?12, thumbnail), asset_count=?13",
            params![
                p.id.to_string(),
                p.name,
                dir.to_string_lossy(),
                p.created_at.to_rfc3339(),
                p.updated_at.to_rfc3339(),
                p.timeline.duration().flicks(),
                p.settings.width,
                p.settings.height,
                p.settings.fps.to_ffmpeg(),
                p.preset_id,
                status,
                thumbnail.map(|t| t.to_string_lossy().into_owned()),
                p.assets.len() as u32,
            ],
        )?;
        Ok(())
    }

    pub fn set_project_status(&self, id: Id, status: &str) -> Result<()> {
        self.conn.execute("UPDATE projects SET status=?2 WHERE id=?1", params![id.to_string(), status])?;
        Ok(())
    }

    pub fn list_projects(&self) -> Result<Vec<ProjectSummary>> {
        let mut st = self.conn.prepare(
            "SELECT id,name,dir,created_at,updated_at,duration,width,height,fps,preset_id,status,thumbnail,asset_count FROM projects ORDER BY updated_at DESC",
        )?;
        let rows = st.query_map([], row_to_summary)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn get_project(&self, id: Id) -> Result<Option<ProjectSummary>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id,name,dir,created_at,updated_at,duration,width,height,fps,preset_id,status,thumbnail,asset_count FROM projects WHERE id=?1",
                [id.to_string()],
                row_to_summary,
            )
            .optional()?)
    }

    pub fn delete_project(&self, id: Id) -> Result<()> {
        self.conn.execute("DELETE FROM projects WHERE id=?1", [id.to_string()])?;
        Ok(())
    }

    // ── jobs ──

    pub fn insert_job(&self, j: &JobRecord) -> Result<()> {
        self.conn.execute(
            "INSERT INTO jobs (id,project_id,kind,title,state,stage,progress,priority,created_at,payload) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                j.id.to_string(),
                j.project_id.map(|i| i.to_string()),
                j.kind,
                j.title,
                j.state,
                j.stage,
                j.progress,
                j.priority,
                j.created_at,
                j.payload.to_string()
            ],
        )?;
        Ok(())
    }

    pub fn update_job(&self, j: &JobRecord) -> Result<()> {
        self.conn.execute(
            "UPDATE jobs SET state=?2, stage=?3, progress=?4, started_at=?5, finished_at=?6, error=?7, result=?8 WHERE id=?1",
            params![j.id.to_string(), j.state, j.stage, j.progress, j.started_at, j.finished_at, j.error, j.result.as_ref().map(|r| r.to_string())],
        )?;
        Ok(())
    }

    pub fn list_jobs(&self, limit: u32) -> Result<Vec<JobRecord>> {
        let mut st = self.conn.prepare(
            "SELECT id,project_id,kind,title,state,stage,progress,priority,created_at,started_at,finished_at,error,payload,result FROM jobs ORDER BY created_at DESC LIMIT ?1",
        )?;
        let rows = st.query_map([limit], row_to_job)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Jobs que estavam em execução/aguardando quando o app fechou.
    pub fn unfinished_jobs(&self) -> Result<Vec<JobRecord>> {
        let mut st = self.conn.prepare(
            "SELECT id,project_id,kind,title,state,stage,progress,priority,created_at,started_at,finished_at,error,payload,result FROM jobs
             WHERE state IN ('queued','running','paused') ORDER BY created_at",
        )?;
        let rows = st.query_map([], row_to_job)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn delete_job(&self, id: Id) -> Result<()> {
        self.conn.execute("DELETE FROM jobs WHERE id=?1", [id.to_string()])?;
        Ok(())
    }

    // ── exportações ──

    pub fn insert_export(&self, e: &ExportRecord) -> Result<()> {
        self.conn.execute(
            "INSERT INTO exports (id,project_id,path,created_at,duration,width,height,codec,size_bytes,report) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                e.id.to_string(),
                e.project_id.to_string(),
                e.path.to_string_lossy(),
                e.created_at,
                e.duration.flicks(),
                e.width,
                e.height,
                e.codec,
                e.size_bytes as i64,
                e.report.to_string()
            ],
        )?;
        Ok(())
    }

    pub fn list_exports(&self, project: Option<Id>) -> Result<Vec<ExportRecord>> {
        let mut st = self.conn.prepare(
            "SELECT id,project_id,path,created_at,duration,width,height,codec,size_bytes,report FROM exports
             WHERE ?1 IS NULL OR project_id=?1 ORDER BY created_at DESC",
        )?;
        let rows = st.query_map([project.map(|p| p.to_string())], |r| {
            Ok(ExportRecord {
                id: parse_id(r.get::<_, String>(0)?),
                project_id: parse_id(r.get::<_, String>(1)?),
                path: PathBuf::from(r.get::<_, String>(2)?),
                created_at: r.get(3)?,
                duration: Time::from_flicks(r.get(4)?),
                width: r.get(5)?,
                height: r.get(6)?,
                codec: r.get(7)?,
                size_bytes: r.get::<_, i64>(8)? as u64,
                report: serde_json::from_str(&r.get::<_, String>(9)?).unwrap_or_default(),
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    // ── configurações ──

    pub fn get_setting<T: serde::de::DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        let v: Option<String> = self.conn.query_row("SELECT value FROM settings WHERE key=?1", [key], |r| r.get(0)).optional()?;
        Ok(match v {
            Some(s) => Some(serde_json::from_str(&s)?),
            None => None,
        })
    }

    pub fn set_setting<T: Serialize>(&self, key: &str, value: &T) -> Result<()> {
        self.conn
            .execute("INSERT INTO settings (key,value) VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET value=?2", params![key, serde_json::to_string(value)?])?;
        Ok(())
    }
}

fn parse_id(s: String) -> Id {
    Id::parse_str(&s).unwrap_or_default()
}

fn row_to_summary(r: &rusqlite::Row) -> rusqlite::Result<ProjectSummary> {
    Ok(ProjectSummary {
        id: parse_id(r.get(0)?),
        name: r.get(1)?,
        dir: PathBuf::from(r.get::<_, String>(2)?),
        created_at: r.get(3)?,
        updated_at: r.get(4)?,
        duration: Time::from_flicks(r.get(5)?),
        width: r.get(6)?,
        height: r.get(7)?,
        fps: r.get(8)?,
        preset_id: r.get(9)?,
        status: r.get(10)?,
        thumbnail: r.get::<_, Option<String>>(11)?.map(PathBuf::from),
        asset_count: r.get(12)?,
    })
}

fn row_to_job(r: &rusqlite::Row) -> rusqlite::Result<JobRecord> {
    Ok(JobRecord {
        id: parse_id(r.get(0)?),
        project_id: r.get::<_, Option<String>>(1)?.map(parse_id),
        kind: r.get(2)?,
        title: r.get(3)?,
        state: r.get(4)?,
        stage: r.get(5)?,
        progress: r.get(6)?,
        priority: r.get(7)?,
        created_at: r.get(8)?,
        started_at: r.get(9)?,
        finished_at: r.get(10)?,
        error: r.get(11)?,
        payload: serde_json::from_str(&r.get::<_, String>(12)?).unwrap_or_default(),
        result: r.get::<_, Option<String>>(13)?.and_then(|s| serde_json::from_str(&s).ok()),
    })
}

pub fn now_rfc3339() -> String {
    now()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wal_projects_settings() {
        let d = tempfile::tempdir().unwrap();
        let db = Db::open(&d.path().join("library.db")).unwrap();
        assert_eq!(db.journal_mode().unwrap(), "wal");
        let p = Project::new("Gameplay", Default::default());
        db.upsert_project(&p, d.path(), None, "idle").unwrap();
        db.upsert_project(&p, d.path(), Some(Path::new("/t.jpg")), "editing").unwrap();
        let l = db.list_projects().unwrap();
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].status, "editing");
        db.set_setting("ai.profile", &"balanced").unwrap();
        assert_eq!(db.get_setting::<String>("ai.profile").unwrap().as_deref(), Some("balanced"));
        // Reabrir mantém dados e não reaplica migrações.
        drop(db);
        let db = Db::open(&d.path().join("library.db")).unwrap();
        assert_eq!(db.list_projects().unwrap().len(), 1);
    }
}
