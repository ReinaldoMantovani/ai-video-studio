//! Sessão de edição com undo/redo.
//!
//! Cada entrada guarda o estado editável (timeline + configurações) antes e
//! depois do comando. Uma timeline típica ocupa poucos KB em memória, então
//! o snapshot completo é mais simples e mais robusto que comandos inversos.

use crate::command::{apply, ApplyOutcome, CommandError, EditCommand};
use crate::model::{Project, ProjectSettings, Timeline};
use serde::Serialize;
use ts_rs::TS;

const MAX_HISTORY: usize = 300;

#[derive(Debug, Clone, PartialEq)]
struct State {
    timeline: Timeline,
    settings: ProjectSettings,
}

#[derive(Debug, Clone)]
struct Entry {
    label: String,
    before: State,
    after: State,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct HistoryInfo {
    pub can_undo: bool,
    pub can_redo: bool,
    pub undo_label: Option<String>,
    pub redo_label: Option<String>,
    /// Rótulos do mais antigo para o mais recente.
    pub entries: Vec<String>,
    /// Incrementa a cada alteração (inclusive undo/redo); útil para invalidação de cache.
    #[ts(type = "number")]
    pub revision: u64,
}

#[derive(Debug, Clone)]
pub struct EditSession {
    pub project: Project,
    undo: Vec<Entry>,
    redo: Vec<Entry>,
    revision: u64,
    saved_revision: u64,
}

impl EditSession {
    pub fn new(project: Project) -> Self {
        EditSession { project, undo: vec![], redo: vec![], revision: 0, saved_revision: 0 }
    }

    fn state(&self) -> State {
        State { timeline: self.project.timeline.clone(), settings: self.project.settings.clone() }
    }
    fn restore(&mut self, s: &State) {
        self.project.timeline = s.timeline.clone();
        self.project.settings = s.settings.clone();
        self.project.updated_at = chrono::Utc::now();
    }

    pub fn execute(&mut self, cmd: &EditCommand) -> Result<ApplyOutcome, CommandError> {
        let before = self.state();
        let out = apply(&mut self.project, cmd)?;
        self.undo.push(Entry { label: out.label.clone(), before, after: self.state() });
        if self.undo.len() > MAX_HISTORY {
            self.undo.remove(0);
        }
        self.redo.clear();
        self.revision += 1;
        Ok(out)
    }

    /// Aplica vários comandos como uma única entrada de histórico (tudo ou nada).
    pub fn execute_batch(&mut self, label: &str, cmds: &[EditCommand]) -> Result<Vec<ApplyOutcome>, CommandError> {
        let before = self.state();
        let snapshot = self.project.clone();
        let mut outs = vec![];
        for c in cmds {
            match apply(&mut self.project, c) {
                Ok(o) => outs.push(o),
                Err(e) => {
                    self.project = snapshot;
                    return Err(e);
                }
            }
        }
        self.undo.push(Entry { label: label.into(), before, after: self.state() });
        self.redo.clear();
        self.revision += 1;
        Ok(outs)
    }

    /// Transação: `f` aplica comandos em sequência (podendo ler o estado
    /// intermediário, ex.: IDs criados por divisões) sobre uma cópia. Se tudo
    /// der certo e o resultado for válido, vira **uma** entrada de histórico.
    pub fn transaction<T>(&mut self, label: &str, f: impl FnOnce(&mut Project) -> Result<T, CommandError>) -> Result<T, CommandError> {
        let before = self.state();
        let mut draft = self.project.clone();
        let out = f(&mut draft)?;
        let issues = crate::validate::validate_project(&draft);
        if !issues.is_empty() {
            return Err(CommandError::Validation(issues));
        }
        self.project = draft;
        self.project.updated_at = chrono::Utc::now();
        self.undo.push(Entry { label: label.into(), before, after: self.state() });
        self.redo.clear();
        self.revision += 1;
        Ok(out)
    }

    /// Substitui timeline e configurações (restaurar versão) como uma única
    /// entrada desfazível. Valida antes de aplicar.
    pub fn replace(&mut self, label: &str, timeline: Timeline, settings: ProjectSettings) -> Result<(), CommandError> {
        let before = self.state();
        let mut draft = self.project.clone();
        draft.timeline = timeline;
        draft.settings = settings;
        let issues = crate::validate::validate_project(&draft);
        if !issues.is_empty() {
            return Err(CommandError::Validation(issues));
        }
        self.project = draft;
        self.project.updated_at = chrono::Utc::now();
        self.undo.push(Entry { label: label.into(), before, after: self.state() });
        self.redo.clear();
        self.revision += 1;
        Ok(())
    }

    pub fn undo(&mut self) -> Option<String> {
        let e = self.undo.pop()?;
        self.restore(&e.before);
        let label = e.label.clone();
        self.redo.push(e);
        self.revision += 1;
        Some(label)
    }

    pub fn redo(&mut self) -> Option<String> {
        let e = self.redo.pop()?;
        self.restore(&e.after);
        let label = e.label.clone();
        self.undo.push(e);
        self.revision += 1;
        Some(label)
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn is_dirty(&self) -> bool {
        self.revision != self.saved_revision
    }
    pub fn mark_saved(&mut self) {
        self.saved_revision = self.revision;
    }
    /// Alterações fora do histórico (ex.: importação de mídia) também sujam o projeto.
    pub fn touch(&mut self) {
        self.revision += 1;
    }

    pub fn info(&self) -> HistoryInfo {
        HistoryInfo {
            can_undo: !self.undo.is_empty(),
            can_redo: !self.redo.is_empty(),
            undo_label: self.undo.last().map(|e| e.label.clone()),
            redo_label: self.redo.last().map(|e| e.label.clone()),
            entries: self.undo.iter().map(|e| e.label.clone()).collect(),
            revision: self.revision,
        }
    }
}
