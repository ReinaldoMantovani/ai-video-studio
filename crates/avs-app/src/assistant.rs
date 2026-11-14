//! Fase 6 — assistente por linguagem natural (camada de serviço).
//!
//! Fluxo: pedido → intents (regras em português ou Ollama local, com JSON
//! Schema) → prévia numa cópia do projeto → o usuário aplica → uma única
//! transação no histórico. O modelo só vê um resumo textual da timeline.

use crate::*;
use avs_ai::assistant::{self, AssistantContext, Intent};
use avs_ai::ollama::Ollama;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AssistantPlan {
    pub text: String,
    /// "regras" ou "ollama:<modelo>".
    pub source: String,
    pub intents: Vec<Intent>,
    /// O que cada ação fará (da execução na cópia).
    pub steps: Vec<String>,
    pub note: Option<String>,
    pub before: Time,
    pub after: Time,
    /// Revisão do projeto em que a prévia foi feita.
    #[ts(type = "number")]
    pub revision: u64,
    /// O que saiu do processo do app para gerar o plano (transparência).
    pub data_sent: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AssistantAvailability {
    /// Interpretador por regras: sempre disponível.
    pub rules: bool,
    /// Modelo local configurado e respondendo.
    pub llm: bool,
    pub llm_model: Option<String>,
    pub llm_reason: Option<String>,
}

impl Studio {
    pub fn assistant_availability(&self) -> AssistantAvailability {
        let s = self.ai_settings();
        let Some(model) = s.ollama_model.clone() else {
            return AssistantAvailability {
                rules: true,
                llm: false,
                llm_model: None,
                llm_reason: Some("Nenhum modelo do Ollama escolhido (Gerenciador de IA → Ollama).".into()),
            };
        };
        let st = Ollama::new(&s.ollama_url).status();
        if !st.running {
            return AssistantAvailability {
                rules: true,
                llm: false,
                llm_model: Some(model),
                llm_reason: Some(st.error.unwrap_or_else(|| "Ollama não está rodando.".into())),
            };
        }
        if !st.models.iter().any(|m| m.name == model) {
            return AssistantAvailability {
                rules: true,
                llm: false,
                llm_model: Some(model.clone()),
                llm_reason: Some(format!("O modelo {model} não está baixado no Ollama.")),
            };
        }
        AssistantAvailability { rules: true, llm: true, llm_model: Some(model), llm_reason: None }
    }

    /// Interpreta e simula o pedido numa cópia do projeto.
    pub fn assistant_plan(&self, project_id: Id, text: &str, ctx: &AssistantContext, use_llm: bool) -> AppResult<AssistantPlan> {
        let text = text.trim();
        if text.chars().count() > 1000 {
            return Err(AppError::new("invalid", "pedido longo demais (máx. 1000 caracteres)"));
        }
        let (p, revision) = {
            let s = self.session(project_id)?;
            let s = s.lock().unwrap();
            (s.project.clone(), s.info().revision)
        };
        let (intents, source, note, data_sent) = if use_llm {
            let a = self.assistant_availability();
            if !a.llm {
                return Err(AppError::new("llm_unavailable", a.llm_reason.unwrap_or_default()));
            }
            let s = self.ai_settings();
            let model = a.llm_model.unwrap();
            let (system, prompt, schema) = assistant::llm_request(text, &p, ctx);
            let r = Ollama::new(&s.ollama_url).generate(&model, &system, &prompt, Some(&schema), 0.1)?;
            let (intents, note) = assistant::parse_llm(&r).map_err(|e| AppError::new("assistant", e))?;
            if intents.is_empty() {
                return Err(AppError::new(
                    "assistant",
                    if note.is_empty() { "O modelo não propôs nenhuma edição.".into() } else { format!("O modelo não propôs edições: {note}") },
                ));
            }
            let sent = format!("Pedido e resumo da timeline (nomes e tempos dos clipes, sem mídia) enviados ao Ollama local em {}.", s.ollama_url);
            (intents, format!("ollama:{model}"), (!note.is_empty()).then_some(note), Some(sent))
        } else {
            let intents = assistant::parse_rules(text, ctx, p.timeline.duration()).map_err(|e| AppError::new("not_understood", e))?;
            (intents, "regras".to_string(), None, None)
        };
        let mut draft = p.clone();
        let map = self.analyses_map(&p);
        let rules = self.auto_edit_rules(project_id)?;
        let done = assistant::execute(&mut draft, &intents, ctx, &map, &rules).map_err(|e| AppError::new("assistant", e))?;
        let issues = validate_project(&draft);
        if let Some(i) = issues.first() {
            return Err(AppError::new("assistant", format!("o resultado seria inválido: {}", i.message)));
        }
        Ok(AssistantPlan {
            text: text.into(),
            source,
            intents,
            steps: done.into_iter().map(|d| d.summary).collect(),
            note,
            before: p.timeline.duration(),
            after: draft.timeline.duration(),
            revision,
            data_sent,
        })
    }

    /// Aplica um plano (de `assistant_plan`) como um único passo no histórico.
    pub fn assistant_apply(&self, project_id: Id, plan: &AssistantPlan, ctx: &AssistantContext) -> AppResult<ProjectState> {
        let rules = self.auto_edit_rules(project_id)?;
        let s = self.session(project_id)?;
        let mut s = s.lock().unwrap();
        if s.info().revision != plan.revision {
            return Err(AppError::new("stale_plan", "o projeto mudou depois da prévia; peça de novo para ver o plano atualizado"));
        }
        let map = self.analyses_map(&s.project);
        let label: String = plan.text.chars().take(48).collect();
        s.transaction(&format!("Assistente: {label}"), |p| assistant::execute(p, &plan.intents, ctx, &map, &rules).map_err(CommandError::Invalid))?;
        s.touch();
        self.autosave(&s)?;
        Ok(state_of(&s, false))
    }
}
