//! Fase 6 — assistente de edição por linguagem natural.
//!
//! O pedido do usuário vira uma lista de [`Intent`]s, um vocabulário pequeno
//! e documentado. Os intents são então convertidos em `EditCommand`s
//! validados. Nada disto executa comandos do sistema, lê arquivos ou acessa a
//! rede: o pior que um pedido (ou um modelo) pode fazer é propor uma edição,
//! que aparece numa prévia e vira um único passo no histórico.
//!
//! Duas formas de interpretar:
//! - [`parse_rules`]: interpretador determinístico de frases em português
//!   ("corte de 0:10 a 0:20", "deixe com 30 segundos", "aumente a música em
//!   3 dB"…). Funciona sem nenhum modelo.
//! - LLM local (Ollama): [`llm_request`] monta o prompt e o JSON Schema que
//!   restringem a resposta a intents; [`parse_llm`] valida o que voltou.
//!   Só o resumo textual da timeline é enviado (nomes, tempos), nunca mídia.

use crate::analysis::AnalysisResult;
use crate::autoedit::{caption_cues, pause_commands, plan_pauses, PauseRules};
use crate::autoplan::{self, AutoEditOptions, PresetRules};
use avs_core::*;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::LazyLock;
use ts_rs::TS;

/// Sobre quais clipes um intent age.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum Target {
    /// Clipe selecionado na timeline.
    Selected,
    /// Clipe de vídeo (ou áudio) sob o cursor.
    AtPlayhead,
    /// Todos os clipes de mídia.
    All,
    /// Áudios sem fala (trilhas sonoras).
    Music,
    /// Clipes com fala (vídeo principal).
    Voice,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "action", rename_all = "snake_case")]
#[ts(export)]
pub enum Intent {
    /// Remove o trecho (todas as trilhas, fechando o buraco).
    RemoveRange {
        start: f64,
        end: f64,
    },
    /// Divide os clipes no instante.
    Split {
        at: f64,
    },
    /// Apaga o clipe-alvo fechando o buraco.
    DeleteClip {
        target: Target,
    },
    /// Remove pausas (silêncios) da fala.
    RemovePauses {
        min_silence_s: Option<f64>,
    },
    /// Gera legendas da transcrição.
    Captions,
    /// Reduz a duração total escolhendo os melhores trechos.
    TargetDuration {
        seconds: f64,
    },
    Speed {
        target: Target,
        factor: f64,
    },
    /// Ajuste de volume relativo em dB.
    Volume {
        target: Target,
        db: f64,
    },
    Mute {
        target: Target,
        mute: bool,
    },
    /// Fade de entrada/saída (`target` = selected/all/music; `where_` = in/out/both).
    Fade {
        target: Target,
        kind: FadeKind,
        seconds: f64,
    },
    AddText {
        text: String,
        at: f64,
        duration: f64,
        position: TextPosition,
    },
    /// Zoom pontual (`amount` em %, ex.: 15 = 115%).
    Zoom {
        at: f64,
        duration: f64,
        amount: f64,
    },
    /// Ajuste de cor relativo (-1..1 para brilho; fatores para contraste/saturação).
    Color {
        target: Target,
        brightness: f64,
        contrast: f64,
        saturation: f64,
    },
    Effect {
        target: Target,
        effect: EffectName,
        amount: f64,
    },
    /// Transições entre clipes encostados (`at` = só o corte mais próximo).
    Transition {
        kind: TransitionKind,
        seconds: f64,
        at: Option<f64>,
    },
    Marker {
        at: f64,
        label: String,
    },
    Freeze {
        at: f64,
        seconds: f64,
    },
    Reverse {
        target: Target,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum FadeKind {
    In,
    Out,
    Both,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TextPosition {
    Top,
    Center,
    Bottom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum EffectName {
    Blur,
    Sharpen,
    Vignette,
    BlackWhite,
    Warm,
    Cool,
}

/// Estado da interface relevante para interpretar o pedido.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AssistantContext {
    pub selected_clip: Option<Id>,
    pub playhead: Time,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AppliedIntent {
    pub intent: Intent,
    /// O que foi feito, em português.
    pub summary: String,
}

// ───────────────────────── interpretador por regras ─────────────────────────

fn deaccent(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'á' | 'à' | 'ã' | 'â' | 'ä' => 'a',
            'é' | 'ê' | 'è' => 'e',
            'í' | 'î' => 'i',
            'ó' | 'õ' | 'ô' | 'ö' => 'o',
            'ú' | 'ü' => 'u',
            'ç' => 'c',
            c => c,
        })
        .collect()
}

static RE_CLOCK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(\d{1,2}):(\d{2})(?::(\d{2}))?(?:[.,](\d+))?\b").unwrap());
static RE_MIN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(\d+(?:[.,]\d+)?)\s*(?:min|minuto|minutos)\b(?:\s*e\s*(\d+(?:[.,]\d+)?)\s*(?:s|seg|segundo|segundos)?\b)?").unwrap());
static RE_SEC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(\d+(?:[.,]\d+)?)\s*(?:s|seg|segs|segundo|segundos)\b").unwrap());
static RE_NUM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(-?\d+(?:[.,]\d+)?)").unwrap());
static RE_QUOTE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"["“”'‘’«»]([^"“”'‘’«»]{1,200})["“”'‘’«»]"#).unwrap());
static RE_SEP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?:;|\.\s+|,?\s+e\s+depois\s+|,?\s+depois\s+|,?\s+em\s+seguida\s+)").unwrap());
/// ", <verbo>" e " e <verbo>" também separam pedidos ("remova as pausas e aumente a música").
static RE_AND_VERB: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:,|,?\s+e)\s+(?:tambem\s+)?((?:remova|remove|tire|tira|corte|corta|apague|apaga|delete|divida|divide|acelere|acelera|desacelere|aumente|aumenta|diminua|diminui|abaixe|abaixa|baixe|suba|silencie|silencia|mute|coloque|coloca|adicione|adiciona|ponha|poe|insira|aplique|aplica|faca|faz|deixe|deixa|gere|gera|crie|cria|congele|congela|inverta|inverte|marque|marca)\b)").unwrap()
});
static RE_TIMES_X: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\d+(?:[.,]\d+)?)\s*(?:x|vezes)").unwrap());

fn split_clauses(t: &str) -> Vec<String> {
    let mut out = vec![];
    for part in RE_SEP.split(t) {
        let mut rest = part;
        while let Some(c) = RE_AND_VERB.captures(rest) {
            let (m, verb) = (c.get(0).unwrap(), c.get(1).unwrap());
            out.push(rest[..m.start()].to_string());
            rest = &rest[verb.start()..];
        }
        out.push(rest.to_string());
    }
    out.into_iter().map(|c| c.trim().trim_end_matches('.').trim().to_string()).filter(|c| !c.is_empty()).collect()
}

fn num(s: &str) -> f64 {
    s.replace(',', ".").parse().unwrap_or(0.0)
}

/// Instantes mencionados, na ordem em que aparecem (segundos).
fn times(t: &str, ctx: &AssistantContext, end: f64) -> Vec<f64> {
    let mut found: Vec<(usize, f64)> = vec![];
    let mut taken: Vec<(usize, usize)> = vec![];
    let free = |taken: &Vec<(usize, usize)>, a: usize, b: usize| !taken.iter().any(|&(x, y)| a < y && b > x);
    for c in RE_CLOCK.captures_iter(t) {
        let m = c.get(0).unwrap();
        let (a, b) = (num(&c[1]), num(&c[2]));
        let mut v = match c.get(3) {
            Some(s) => a * 3600.0 + b * 60.0 + num(s.as_str()),
            None => a * 60.0 + b,
        };
        if let Some(f) = c.get(4) {
            v += num(&format!("0.{}", f.as_str()));
        }
        found.push((m.start(), v));
        taken.push((m.start(), m.end()));
    }
    for c in RE_MIN.captures_iter(t) {
        let m = c.get(0).unwrap();
        if free(&taken, m.start(), m.end()) {
            found.push((m.start(), num(&c[1]) * 60.0 + c.get(2).map(|x| num(x.as_str())).unwrap_or(0.0)));
            taken.push((m.start(), m.end()));
        }
    }
    for c in RE_SEC.captures_iter(t) {
        let m = c.get(0).unwrap();
        if free(&taken, m.start(), m.end()) {
            found.push((m.start(), num(&c[1])));
            taken.push((m.start(), m.end()));
        }
    }
    for (kw, v) in [
        ("no cursor", ctx.playhead.as_secs_f64()),
        ("aqui", ctx.playhead.as_secs_f64()),
        ("neste ponto", ctx.playhead.as_secs_f64()),
        ("este ponto", ctx.playhead.as_secs_f64()),
        ("no inicio", 0.0),
        ("no comeco", 0.0),
        ("no final", end),
        ("no fim", end),
    ] {
        if let Some(i) = t.find(kw) {
            found.push((i, v));
        }
    }
    found.sort_by_key(|x| x.0);
    found.into_iter().map(|x| x.1).collect()
}

/// Durações explícitas ("por 3 segundos", "de 2 s").
fn duration_after(t: &str, kws: &[&str]) -> Option<f64> {
    for kw in kws {
        if let Some(i) = t.find(kw) {
            let rest = &t[i + kw.len()..];
            if let Some(c) = RE_SEC.captures(rest).filter(|c| c.get(0).unwrap().start() < 6) {
                return Some(num(&c[1]));
            }
        }
    }
    None
}

fn target_of(t: &str, ctx: &AssistantContext) -> Target {
    if t.contains("musica") || t.contains("trilha sonora") || t.contains("fundo musical") {
        Target::Music
    } else if t.contains("voz") || t.contains("fala") || t.contains("dialogo") {
        Target::Voice
    } else if t.contains("selecionad") || t.contains("este clipe") || t.contains("esse clipe") || t.contains("clipe atual") {
        Target::Selected
    } else if t.contains("todos") || t.contains("todo o video") || t.contains("video todo") || t.contains("tudo") || t.contains("video inteiro") {
        Target::All
    } else if t.contains("clipe") && (t.contains("cursor") || t.contains("aqui")) {
        Target::AtPlayhead
    } else if ctx.selected_clip.is_some() {
        Target::Selected
    } else {
        // Sem alvo explícito e nada selecionado: o vídeo todo.
        Target::All
    }
}

fn first_number(t: &str) -> Option<f64> {
    RE_NUM.captures(t).map(|c| num(&c[1]))
}

/// Interpreta um pedido em português. `Err` explica o que não foi entendido.
pub fn parse_rules(text: &str, ctx: &AssistantContext, timeline_end: Time) -> Result<Vec<Intent>, String> {
    let raw = text.trim();
    if raw.is_empty() {
        return Err("Escreva o que quer fazer.".into());
    }
    // Texto entre aspas é preservado (texto na tela, nome de marcador).
    let quoted: Vec<String> = RE_QUOTE.captures_iter(raw).map(|c| c[1].trim().to_string()).collect();
    let lower = deaccent(&RE_QUOTE.replace_all(&raw.to_lowercase(), "\"\"")).replace("  ", " ");
    let end = timeline_end.as_secs_f64();
    let mut out = vec![];
    let mut q = quoted.into_iter();
    for clause in split_clauses(&lower) {
        let c = clause.as_str();
        let ts = times(c, ctx, end);
        let has = |ws: &[&str]| ws.iter().any(|w| c.contains(w));
        let intent = if has(&["pausa", "silencio", "respiro"]) && has(&["remov", "tir", "cort", "elimin"]) {
            Intent::RemovePauses { min_silence_s: duration_after(c, &["maiores que", "acima de", "mais de", "de"]) }
        } else if has(&["legenda"]) && has(&["ger", "cri", "adicion", "coloc", "ponh", "faz", "faca", "insir"]) {
            Intent::Captions
        } else if has(&["deix", "reduz", "encurt", "resum", "ajust"])
            && has(&["com ", "para ", "em "])
            && !ts.is_empty()
            && has(&["video", "duracao", "segundo", "minuto", " s", "seg"])
            && !has(&["volume", "fade", "zoom", "texto"])
        {
            Intent::TargetDuration { seconds: ts[0] }
        } else if has(&["transic"]) {
            let kind = if has(&["preto"]) {
                TransitionKind::FadeBlack
            } else if has(&["cortina", "wipe"]) {
                TransitionKind::WipeLeft
            } else if has(&["desliz", "slide"]) {
                TransitionKind::SlideLeft
            } else if has(&["dissolv"]) {
                TransitionKind::Dissolve
            } else {
                TransitionKind::Crossfade
            };
            let seconds = duration_after(c, &["de ", "com "]).unwrap_or(0.5);
            let at = if has(&["todos", "todas", "entre os clipes", "cada corte"]) { None } else { ts.iter().copied().find(|t| (*t - seconds).abs() > 1e-6) };
            Intent::Transition { kind, seconds, at }
        } else if has(&["fade", "esmaec"]) || (has(&["sumir", "aparecer"]) && has(&["aos poucos", "suave", "gradual"])) {
            let kind = if has(&["entrada", " in", "comeco", "inicio", "aparec"]) && has(&["saida", " out", "final", "fim", "sumir"]) {
                FadeKind::Both
            } else if has(&["entrada", " in", "comeco", "inicio", "aparec"]) {
                FadeKind::In
            } else if has(&["saida", " out", "final", "fim", "sumir"]) {
                FadeKind::Out
            } else {
                FadeKind::Both
            };
            let target = if has(&["musica", "trilha sonora"]) {
                Target::Music
            } else if has(&["selecionad", "clipe"]) {
                Target::Selected
            } else {
                Target::All
            };
            Intent::Fade {
                target,
                kind,
                seconds: duration_after(c, &["de ", "com ", "por "]).or_else(|| RE_SEC.captures(c).map(|x| num(&x[1]))).unwrap_or(1.0),
            }
        } else if has(&["silenci", "mute", "mutar", "sem som", "tire o som", "tira o som", "tire o audio"]) {
            Intent::Mute { target: target_of(c, ctx), mute: !has(&["dessilenci", "desmut", "volte o som", "reativ"]) }
        } else if has(&["volume", "mais alto", "mais baixo", "abaix", "aument", "diminu", "suba", "baixe"])
            && !has(&["velocidade", "zoom", "brilho", "contraste", "saturac"])
        {
            let up = has(&["aument", "mais alto", "suba", "subir", "levant"]);
            let amount = RE_NUM.captures_iter(c).filter(|x| c[x.get(0).unwrap().end()..].trim_start().starts_with("db")).map(|x| num(&x[1]).abs()).next();
            let pct = RE_NUM.captures_iter(c).filter(|x| c[x.get(0).unwrap().end()..].trim_start().starts_with('%')).map(|x| num(&x[1]).abs()).next();
            let db = amount.or_else(|| pct.map(|p| 20.0 * (1.0 + p / 100.0f64).log10() * if up { 1.0 } else { -1.0 }).map(f64::abs)).unwrap_or(if up {
                3.0
            } else {
                6.0
            });
            Intent::Volume { target: target_of(c, ctx), db: if up { db } else { -db } }
        } else if has(&["velocidade", "aceler", "desaceler", "camera lenta", "camera rapida", "mais rapido", "mais devagar", "slow"]) {
            let x = RE_TIMES_X.captures(c).map(|m| num(&m[1]));
            let pct = RE_NUM.captures_iter(c).filter(|m| c[m.get(0).unwrap().end()..].trim_start().starts_with('%')).map(|m| num(&m[1])).next();
            let slow = has(&["desaceler", "camera lenta", "devagar", "slow", "reduz"]);
            let factor = x.or_else(|| pct.map(|p| if slow { 1.0 - p / 100.0 } else { 1.0 + p / 100.0 })).unwrap_or(if slow { 0.5 } else { 1.5 });
            let factor = if slow && factor > 1.0 { 1.0 / factor } else { factor };
            Intent::Speed { target: target_of(c, ctx), factor }
        } else if has(&["zoom", "aproxim", "close"]) {
            let at = ts.first().copied().unwrap_or(ctx.playhead.as_secs_f64());
            let duration = duration_after(c, &["por ", "durante "]).unwrap_or(2.0);
            let amount =
                RE_NUM.captures_iter(c).filter(|m| c[m.get(0).unwrap().end()..].trim_start().starts_with('%')).map(|m| num(&m[1])).next().unwrap_or(15.0);
            Intent::Zoom { at, duration, amount }
        } else if has(&["texto", "titulo", "escrev", "letreiro", "chamada"]) {
            let text = q.next().ok_or("Coloque o texto entre aspas, por exemplo: adicione o texto \"Inscreva-se\" em 0:05.")?;
            let duration = duration_after(c, &["por ", "durante "]).unwrap_or(3.0);
            let at = ts.iter().copied().find(|t| (*t - duration).abs() > 1e-6 || ts.len() == 1).unwrap_or(ctx.playhead.as_secs_f64());
            let position = if has(&["topo", "em cima", "superior", "alto da tela"]) {
                TextPosition::Top
            } else if has(&["centro", "meio"]) {
                TextPosition::Center
            } else {
                TextPosition::Bottom
            };
            Intent::AddText { text, at, duration, position }
        } else if has(&["marcador", "marque", "marca "]) {
            let label = q.next().unwrap_or_else(|| "Marcador".into());
            Intent::Marker { at: ts.first().copied().unwrap_or(ctx.playhead.as_secs_f64()), label }
        } else if has(&["congel", "freeze", "pause a imagem", "pausar a imagem"]) {
            let seconds = duration_after(c, &["por ", "durante "]).unwrap_or(2.0);
            Intent::Freeze { at: ts.iter().copied().find(|t| (*t - seconds).abs() > 1e-6).unwrap_or(ctx.playhead.as_secs_f64()), seconds }
        } else if has(&["invert", "revers", "de tras pra frente", "de tras para frente", "ao contrario"]) {
            Intent::Reverse { target: target_of(c, ctx) }
        } else if has(&["preto e branco", "p&b", "pb ", "sem cor", "desatur"]) {
            Intent::Effect { target: target_of(c, ctx), effect: EffectName::BlackWhite, amount: 1.0 }
        } else if has(&["desfoq", "blur", "embac"]) {
            Intent::Effect { target: target_of(c, ctx), effect: EffectName::Blur, amount: first_number(c).unwrap_or(8.0) }
        } else if has(&["nitidez", "nitido"]) {
            Intent::Effect { target: target_of(c, ctx), effect: EffectName::Sharpen, amount: 0.8 }
        } else if has(&["vinheta"]) {
            Intent::Effect { target: target_of(c, ctx), effect: EffectName::Vignette, amount: 0.5 }
        } else if has(&["quente", "aquec"]) {
            Intent::Effect { target: target_of(c, ctx), effect: EffectName::Warm, amount: 1.0 }
        } else if has(&["fria", "frio", "esfri", "azulad"]) {
            Intent::Effect { target: target_of(c, ctx), effect: EffectName::Cool, amount: 1.0 }
        } else if has(&["brilho", "claro", "clare", "escur", "contraste", "saturac", "cores mais vivas", "mais vivo", "mais vibrante"]) {
            let up = !has(&["escur", "menos", "diminu", "reduz", "abaix"]);
            let s = if up { 1.0 } else { -1.0 };
            let (mut b, mut ct, mut sat) = (0.0, 1.0, 1.0);
            if has(&["brilho", "claro", "clare", "escur"]) {
                b = 0.08 * s;
            }
            if has(&["contraste"]) {
                ct = 1.0 + 0.15 * s;
            }
            if has(&["saturac", "vivas", "vivo", "vibrante"]) {
                sat = 1.0 + 0.25 * s;
            }
            Intent::Color { target: target_of(c, ctx), brightness: b, contrast: ct, saturation: sat }
        } else if has(&["divid", "separ", "quebr", "split"]) || (has(&["cort", "corta"]) && ts.len() == 1 && !has(&[" ate ", " a ", " ao ", "entre", "trecho"]))
        {
            Intent::Split { at: ts.first().copied().unwrap_or(ctx.playhead.as_secs_f64()) }
        } else if has(&["remov", "tir", "cort", "apag", "delet", "elimin", "exclu"]) && ts.len() >= 2 {
            let (a, b) = (ts[0].min(ts[1]), ts[0].max(ts[1]));
            Intent::RemoveRange { start: a, end: b }
        } else if has(&["remov", "tir", "apag", "delet", "elimin", "exclu"]) && has(&["clipe", "selecionad", "este", "esse"]) {
            Intent::DeleteClip { target: if has(&["cursor", "aqui"]) { Target::AtPlayhead } else { Target::Selected } }
        } else if has(&["remov", "tir", "cort", "apag"]) && ts.len() == 1 && has(&["primeiros", "inicio", "comeco"]) {
            Intent::RemoveRange { start: 0.0, end: ts[0] }
        } else if has(&["remov", "tir", "cort", "apag"]) && ts.len() == 1 && has(&["ultimos", "final", "fim"]) {
            Intent::RemoveRange { start: (end - ts[0]).max(0.0), end }
        } else {
            return Err(format!("Não entendi \"{clause}\". Exemplos: \"remova as pausas\", \"corte de 0:10 a 0:20\", \"deixe com 30 segundos\", \"aumente a música em 3 dB\", \"adicione o texto \\\"Inscreva-se\\\" em 0:05\"."));
        };
        out.push(intent);
    }
    Ok(out)
}

// ───────────────────────────── LLM local (Ollama) ─────────────────────────────

/// Resumo textual da timeline para o modelo (sem mídia, sem caminhos).
pub fn timeline_summary(p: &Project, ctx: &AssistantContext) -> String {
    let mut s = format!(
        "Projeto \"{}\", {}x{}, duração {:.2} s, cursor em {:.2} s.\n",
        p.name,
        p.settings.width,
        p.settings.height,
        p.timeline.duration().as_secs_f64(),
        ctx.playhead.as_secs_f64()
    );
    for t in &p.timeline.tracks {
        s.push_str(&format!("Trilha {:?} \"{}\":\n", t.kind, t.name));
        for c in t.clips.iter().take(60) {
            let sel = if Some(c.id) == ctx.selected_clip { " [SELECIONADO]" } else { "" };
            s.push_str(&format!("  - \"{}\" de {:.2} s a {:.2} s{sel}\n", c.name, c.start.as_secs_f64(), c.end().as_secs_f64()));
        }
    }
    s
}

/// (system, prompt, schema) para `Ollama::generate`.
pub fn llm_request(text: &str, p: &Project, ctx: &AssistantContext) -> (String, String, Value) {
    let system = "Você converte pedidos de edição de vídeo em ações JSON. Use somente as ações do schema. \
Tempos em segundos na timeline. 'target': selected (clipe selecionado), at_playhead, all, music (trilhas sonoras), voice (fala). \
Se o pedido não for uma edição de vídeo ou não puder ser atendido com essas ações, devolva actions vazio e explique em 'note'. \
Nunca invente arquivos, comandos do sistema ou ações fora da lista."
        .to_string();
    let prompt = format!("{}\nPedido: {text}", timeline_summary(p, ctx));
    let target = json!({"type": "string", "enum": ["selected", "at_playhead", "all", "music", "voice"]});
    let num = json!({"type": "number"});
    let action = |name: &str, props: Value, req: &[&str]| {
        let mut pr = props.as_object().cloned().unwrap_or_default();
        pr.insert("action".into(), json!({"const": name}));
        let mut r: Vec<&str> = vec!["action"];
        r.extend(req);
        json!({"type": "object", "properties": pr, "required": r, "additionalProperties": false})
    };
    let schema = json!({
        "type": "object",
        "properties": {
            "actions": {"type": "array", "maxItems": 12, "items": {"anyOf": [
                action("remove_range", json!({"start": num, "end": num}), &["start", "end"]),
                action("split", json!({"at": num}), &["at"]),
                action("delete_clip", json!({"target": target}), &["target"]),
                action("remove_pauses", json!({"min_silence_s": {"type": ["number", "null"]}}), &[]),
                action("captions", json!({}), &[]),
                action("target_duration", json!({"seconds": num}), &["seconds"]),
                action("speed", json!({"target": target, "factor": num}), &["target", "factor"]),
                action("volume", json!({"target": target, "db": num}), &["target", "db"]),
                action("mute", json!({"target": target, "mute": {"type": "boolean"}}), &["target", "mute"]),
                action("fade", json!({"target": target, "kind": {"enum": ["in", "out", "both"]}, "seconds": num}), &["target", "kind", "seconds"]),
                action("add_text", json!({"text": {"type": "string"}, "at": num, "duration": num, "position": {"enum": ["top", "center", "bottom"]}}), &["text", "at", "duration", "position"]),
                action("zoom", json!({"at": num, "duration": num, "amount": num}), &["at", "duration", "amount"]),
                action("color", json!({"target": target, "brightness": num, "contrast": num, "saturation": num}), &["target", "brightness", "contrast", "saturation"]),
                action("effect", json!({"target": target, "effect": {"enum": ["blur", "sharpen", "vignette", "black_white", "warm", "cool"]}, "amount": num}), &["target", "effect", "amount"]),
                action("transition", json!({"kind": {"enum": ["crossfade", "fade_black", "wipe_left", "slide_left", "dissolve"]}, "seconds": num, "at": {"type": ["number", "null"]}}), &["kind", "seconds"]),
                action("marker", json!({"at": num, "label": {"type": "string"}}), &["at", "label"]),
                action("freeze", json!({"at": num, "seconds": num}), &["at", "seconds"]),
                action("reverse", json!({"target": target}), &["target"])
            ]}},
            "note": {"type": "string"}
        },
        "required": ["actions", "note"]
    });
    (system, prompt, schema)
}

/// Valida a resposta do modelo. Ações desconhecidas ou malformadas são
/// recusadas com o motivo (não são "consertadas" silenciosamente).
pub fn parse_llm(response: &str) -> Result<(Vec<Intent>, String), String> {
    let v: Value = serde_json::from_str(response).map_err(|e| format!("o modelo não devolveu JSON válido ({e})"))?;
    let note = v["note"].as_str().unwrap_or("").to_string();
    let arr = v["actions"].as_array().ok_or("resposta sem a lista 'actions'")?;
    if arr.len() > 12 {
        return Err("o modelo propôs ações demais (máximo 12)".into());
    }
    let mut out = vec![];
    for a in arr {
        let mut a = a.clone();
        // "captions" sem campos chega como objeto só com "action".
        let i: Intent = serde_json::from_value(a.take()).map_err(|e| format!("ação inválida do modelo: {e}"))?;
        out.push(i);
    }
    Ok((out, note))
}

// ───────────────────────────── execução ─────────────────────────────

static RE_DEC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\d)\.(\d)").unwrap());

/// "1.5 s" → "1,5 s", sem mexer no texto do usuário (entre aspas).
fn pt_decimals(s: &str) -> String {
    s.split('"')
        .enumerate()
        .map(|(i, part)| if i % 2 == 0 { RE_DEC.replace_all(part, "$1,$2").into_owned() } else { part.to_string() })
        .collect::<Vec<_>>()
        .join("\"")
}

fn t(s: f64) -> Time {
    Time::from_secs_f64(s.max(0.0))
}

fn media_clips(p: &Project) -> Vec<(TrackKind, Clip)> {
    p.timeline.tracks.iter().filter(|t| t.kind != TrackKind::Text && !t.locked).flat_map(|t| t.clips.iter().map(move |c| (t.kind, c.clone()))).collect()
}

fn is_music(p: &Project, kind: TrackKind, c: &Clip) -> bool {
    kind == TrackKind::Audio && c.asset_id().and_then(|id| p.asset(id)).map(|a| !a.has_video()).unwrap_or(false)
}

fn resolve(p: &Project, ctx: &AssistantContext, target: Target, needs_video: bool) -> Result<Vec<Id>, String> {
    let fps = p.settings.fps;
    let ids: Vec<Id> = match target {
        Target::Selected => {
            let id = ctx.selected_clip.ok_or("Selecione um clipe na timeline (ou diga \"todos\", \"a música\" ou \"a fala\").")?;
            p.timeline.clip(id).ok_or("O clipe selecionado não existe mais.")?;
            vec![id]
        }
        Target::AtPlayhead => {
            let at = ctx.playhead.snap_to_frame(fps);
            let c = media_clips(p).into_iter().filter(|(k, c)| c.range().contains(at) && (!needs_video || *k == TrackKind::Video)).map(|(_, c)| c.id).next();
            vec![c.ok_or("Não há clipe sob o cursor.")?]
        }
        Target::All => media_clips(p).into_iter().filter(|(k, _)| !needs_video || *k == TrackKind::Video).map(|(_, c)| c.id).collect(),
        Target::Music => media_clips(p).into_iter().filter(|(k, c)| is_music(p, *k, c)).map(|(_, c)| c.id).collect(),
        Target::Voice => media_clips(p)
            .into_iter()
            .filter(|(k, c)| *k == TrackKind::Video && c.asset_id().and_then(|id| p.asset(id)).map(|a| a.has_audio()).unwrap_or(false))
            .map(|(_, c)| c.id)
            .collect(),
    };
    if ids.is_empty() {
        return Err(match target {
            Target::Music => "Não há música (áudio sem vídeo) na timeline.".into(),
            Target::Voice => "Não há clipes com fala na timeline.".into(),
            _ => "Nenhum clipe para alterar.".into(),
        });
    }
    Ok(ids)
}

fn label(target: Target, n: usize) -> String {
    match target {
        Target::Selected => "no clipe selecionado".into(),
        Target::AtPlayhead => "no clipe sob o cursor".into(),
        Target::All => format!("em {n} clipe(s)"),
        Target::Music => format!("na música ({n} clipe(s))"),
        Target::Voice => format!("na fala ({n} clipe(s))"),
    }
}

fn ap(p: &mut Project, c: EditCommand) -> Result<ApplyOutcome, String> {
    apply(p, &c).map_err(|e| e.to_string())
}

fn patch(p: &mut Project, id: Id, f: impl FnOnce(&Clip, &mut ClipPatch)) -> Result<(), String> {
    let c = p.timeline.clip(id).ok_or("clipe não encontrado")?.clone();
    let mut pt = ClipPatch::default();
    f(&c, &mut pt);
    ap(p, EditCommand::UpdateClip { clip_id: id, patch: pt }).map(|_| ())
}

/// Aplica os intents em ordem sobre `p` (normalmente dentro de uma transação
/// ou numa cópia para a prévia). `rules` vem do preset do projeto.
pub fn execute(
    p: &mut Project,
    intents: &[Intent],
    ctx: &AssistantContext,
    analyses: &HashMap<Id, AnalysisResult>,
    rules: &PresetRules,
) -> Result<Vec<AppliedIntent>, String> {
    let fps = p.settings.fps;
    let mut done = vec![];
    for (k, i) in intents.iter().enumerate() {
        let n = k + 1;
        let err = |e: String| format!("Passo {n}: {e}");
        let end = p.timeline.duration().as_secs_f64();
        let check_t = |x: f64| -> Result<(), String> {
            if !(0.0..=end + 1e-6).contains(&x) {
                Err(format!("o instante {x:.2} s está fora do vídeo (0–{end:.2} s)"))
            } else {
                Ok(())
            }
        };
        let summary: String = match i {
            Intent::RemoveRange { start, end: e } => {
                check_t(*start).and(check_t(e.min(end))).map_err(err)?;
                let (a, b) = (t(*start).snap_to_frame(fps), t(e.min(end)).snap_to_frame(fps));
                if b <= a {
                    return Err(err("o trecho está vazio".into()));
                }
                ap(p, EditCommand::DeleteRange { start: a, end: b, track_ids: None, ripple: true }).map_err(err)?;
                format!("Removido o trecho de {:.2} s a {:.2} s ({:.2} s)", a.as_secs_f64(), b.as_secs_f64(), (b - a).as_secs_f64())
            }
            Intent::Split { at } => {
                check_t(*at).map_err(err)?;
                let at_t = t(*at).snap_to_frame(fps);
                let ids: Vec<Id> = p
                    .timeline
                    .tracks
                    .iter()
                    .filter(|t| !t.locked)
                    .flat_map(|t| t.clips.iter())
                    .filter(|c| at_t > c.start && at_t < c.end())
                    .map(|c| c.id)
                    .collect();
                if ids.is_empty() {
                    return Err(err(format!("não há clipe para dividir em {at:.2} s")));
                }
                for id in &ids {
                    ap(p, EditCommand::SplitClip { clip_id: *id, at: at_t }).map_err(err)?;
                }
                format!("Dividido em {:.2} s ({} clipe(s))", at_t.as_secs_f64(), ids.len())
            }
            Intent::DeleteClip { target } => {
                let ids = resolve(p, ctx, *target, false).map_err(err)?;
                for id in &ids {
                    ap(p, EditCommand::DeleteClip { clip_id: *id, ripple: true }).map_err(err)?;
                }
                format!("Apagado {} (buraco fechado)", label(*target, ids.len()))
            }
            Intent::RemovePauses { min_silence_s } => {
                let mut pr = PauseRules {
                    min_silence: Time::from_millis(rules.min_silence_ms as i64),
                    padding: Time::from_millis(rules.padding_ms as i64),
                    preserve_action: rules.preserve_silent_action,
                    ..Default::default()
                };
                if let Some(m) = min_silence_s {
                    pr.min_silence = t(*m);
                }
                let plan = plan_pauses(p, analyses, None, &pr);
                if plan.cuts.is_empty() {
                    return Err(err(if analyses.is_empty() {
                        "analise as mídias antes (aba IA → Analisar mídias)".into()
                    } else {
                        "nenhuma pausa encontrada com essa duração mínima".into()
                    }));
                }
                let total = plan.removed;
                // Legendas e textos acompanham os cortes.
                let sync: Vec<Id> = p.timeline.tracks.iter().filter(|t| t.kind == TrackKind::Text && !t.locked).map(|t| t.id).collect();
                for c in pause_commands(&plan, &sync) {
                    ap(p, c).map_err(err)?;
                }
                format!("{} pausa(s) removida(s), {:.1} s no total", plan.cuts.len(), total.as_secs_f64())
            }
            Intent::Captions => {
                let mut cues = vec![];
                for (kind, c) in media_clips(p) {
                    if kind != TrackKind::Video {
                        continue;
                    }
                    if let Some(tr) = c.asset_id().and_then(|id| analyses.get(&id)).and_then(|a| a.transcript.as_ref()) {
                        cues.extend(caption_cues(tr, &c, 42, Time::from_secs(4)));
                    }
                }
                if cues.is_empty() {
                    return Err(err("não há transcrição: instale a transcrição e analise com \"Transcrever fala\"".into()));
                }
                let n = cues.len();
                let mut style = subtitles::caption_style();
                style.y = rules.caption_y;
                style.size = rules.caption_size;
                ap(p, EditCommand::AddSubtitleTrack { name: Some("Legendas".into()), cues, style: Some(style) }).map_err(err)?;
                format!("{n} legendas criadas a partir da transcrição")
            }
            Intent::TargetDuration { seconds } => {
                if *seconds < 3.0 {
                    return Err(err("duração muito curta".into()));
                }
                if *seconds >= end {
                    return Err(err(format!("o vídeo já tem {end:.1} s; nada é esticado artificialmente")));
                }
                let mut r = rules.clone();
                r.narrative = if r.narrative == "preserve" { "best_moments".into() } else { r.narrative.clone() };
                let opts = AutoEditOptions {
                    target_duration: Some(t(*seconds)),
                    narrative: Some(r.narrative.clone()),
                    transitions: false,
                    zoom: false,
                    audio_treatment: false,
                    ducking: false,
                    color_correction: false,
                    captions: Some(false),
                    remove_fillers: Some(false),
                    ..Default::default()
                };
                let rep = autoplan::run(p, &r, analyses, &opts).map_err(|e| err(e.to_string()))?;
                let mut s = format!(
                    "Duração reduzida de {:.1} s para {:.1} s mantendo os trechos de maior nota, em ordem",
                    rep.before.as_secs_f64(),
                    rep.after.as_secs_f64()
                );
                for w in rep.warnings {
                    s.push_str(&format!(" ({w})"));
                }
                s
            }
            Intent::Speed { target, factor } => {
                if !(0.25..=4.0).contains(factor) {
                    return Err(err(format!("velocidade {factor}× fora do limite (0,25× a 4×)")));
                }
                let ids = resolve(p, ctx, *target, false).map_err(err)?;
                for id in &ids {
                    ap(p, EditCommand::SetClipSpeed { clip_id: *id, speed: *factor as f32, ripple: true }).map_err(err)?;
                }
                format!("Velocidade {:.2}× {}", factor, label(*target, ids.len()))
            }
            Intent::Volume { target, db } => {
                let ids = resolve(p, ctx, *target, false).map_err(err)?;
                let k = 10f64.powf(db / 20.0) as f32;
                for id in &ids {
                    patch(p, *id, |c, pt| pt.volume = Some((c.volume * k).clamp(0.0, 4.0))).map_err(err)?;
                }
                format!("Volume {:+.1} dB {}", db, label(*target, ids.len()))
            }
            Intent::Mute { target, mute } => {
                let ids = resolve(p, ctx, *target, false).map_err(err)?;
                for id in &ids {
                    patch(p, *id, |_, pt| pt.audio_muted = Some(*mute)).map_err(err)?;
                }
                format!("{} {}", if *mute { "Som desligado" } else { "Som religado" }, label(*target, ids.len()))
            }
            Intent::Fade { target, kind, seconds } => {
                let d = t(seconds.clamp(0.1, 10.0)).snap_to_frame(fps);
                let ids: Vec<Id> = match target {
                    // "Fade no vídeo": só o primeiro (entrada) e o último (saída) clipe de cada trilha.
                    Target::All => {
                        let mut v = vec![];
                        for tr in p.timeline.tracks.iter().filter(|t| t.kind != TrackKind::Text && !t.locked) {
                            if let (Some(f), Some(l)) = (tr.clips.first(), tr.clips.last()) {
                                if matches!(kind, FadeKind::In | FadeKind::Both) {
                                    v.push((f.id, true));
                                }
                                if matches!(kind, FadeKind::Out | FadeKind::Both) {
                                    v.push((l.id, false));
                                }
                            }
                        }
                        for (id, is_in) in &v {
                            patch(p, *id, |c, pt| {
                                let d = d.min(c.duration / 2);
                                if *is_in {
                                    pt.fade_in = Some(d)
                                } else {
                                    pt.fade_out = Some(d)
                                }
                            })
                            .map_err(err)?;
                        }
                        v.iter().map(|x| x.0).collect()
                    }
                    _ => {
                        let ids = resolve(p, ctx, *target, false).map_err(err)?;
                        for id in &ids {
                            patch(p, *id, |c, pt| {
                                let d = d.min(c.duration / 2);
                                if matches!(kind, FadeKind::In | FadeKind::Both) {
                                    pt.fade_in = Some(d);
                                }
                                if matches!(kind, FadeKind::Out | FadeKind::Both) {
                                    pt.fade_out = Some(d);
                                }
                            })
                            .map_err(err)?;
                        }
                        ids
                    }
                };
                let k = match kind {
                    FadeKind::In => "de entrada",
                    FadeKind::Out => "de saída",
                    FadeKind::Both => "de entrada e saída",
                };
                format!("Fade {k} de {:.1} s ({} clipe(s))", d.as_secs_f64(), ids.len())
            }
            Intent::AddText { text, at, duration, position } => {
                check_t(*at).map_err(err)?;
                if text.trim().is_empty() || text.chars().count() > 200 {
                    return Err(err("texto vazio ou longo demais (máx. 200 caracteres)".into()));
                }
                let start = t(*at).snap_to_frame(fps);
                let dur = t(duration.clamp(0.5, 60.0)).snap_to_frame(fps);
                // Trilha de texto livre no intervalo (ou uma nova).
                let range = TimeRange::new(start, start + dur);
                let track = p
                    .timeline
                    .tracks
                    .iter()
                    .find(|t| t.kind == TrackKind::Text && !t.locked && !t.clips.iter().any(|c| c.range().intersects(&range)))
                    .map(|t| t.id);
                let track = match track {
                    Some(t) => t,
                    None => ap(p, EditCommand::AddTrack { kind: TrackKind::Text, name: Some("Textos do assistente".into()) }).map_err(err)?.created[0],
                };
                let style = TextStyle {
                    y: match position {
                        TextPosition::Top => 0.12,
                        TextPosition::Center => 0.5,
                        TextPosition::Bottom => (1.0 - rules.safe_bottom_pct - 0.08).clamp(0.6, 0.9),
                    },
                    ..Default::default()
                };
                ap(p, EditCommand::AddText { track_id: track, start, duration: dur, text: text.clone(), style: Some(style) }).map_err(err)?;
                format!("Texto \"{}\" em {:.2} s por {:.1} s", text, start.as_secs_f64(), dur.as_secs_f64())
            }
            Intent::Zoom { at, duration, amount } => {
                check_t(*at).map_err(err)?;
                let at_t = t(*at).snap_to_frame(fps);
                let c = p
                    .timeline
                    .tracks
                    .iter()
                    .filter(|t| t.kind == TrackKind::Video && !t.locked)
                    .flat_map(|t| t.clips.iter())
                    .find(|c| c.range().contains(at_t) && c.asset_id().is_some())
                    .cloned()
                    .ok_or_else(|| err(format!("não há vídeo em {at:.2} s")))?;
                let z = (1.0 + amount.clamp(2.0, 100.0) / 100.0) as f32;
                let rel = at_t - c.start;
                let dur = t(duration.clamp(0.4, 30.0));
                let ramp = Time::from_millis(400);
                let pts =
                    [(rel - ramp, 1.0, Easing::EaseInOut), (rel, z, Easing::Hold), (rel + dur, z, Easing::EaseInOut), (rel + dur + ramp, 1.0, Easing::Linear)];
                for (tt, v, e) in pts {
                    if tt >= Time::ZERO && tt <= c.duration {
                        ap(
                            p,
                            EditCommand::SetKeyframe { clip_id: c.id, property: KeyframeProperty::Scale, at: tt.snap_to_frame(fps), value: v, easing: Some(e) },
                        )
                        .map_err(err)?;
                    }
                }
                format!("Zoom de {:.0}% em {:.2} s por {:.1} s", amount, at_t.as_secs_f64(), dur.as_secs_f64())
            }
            Intent::Color { target, brightness, contrast, saturation } => {
                let ids = resolve(p, ctx, *target, true).map_err(err)?;
                for id in &ids {
                    patch(p, *id, |c, pt| {
                        pt.color = Some(ColorAdjust {
                            brightness: (c.color.brightness + *brightness as f32).clamp(-1.0, 1.0),
                            contrast: (c.color.contrast * *contrast as f32).clamp(0.0, 3.0),
                            saturation: (c.color.saturation * *saturation as f32).clamp(0.0, 3.0),
                        })
                    })
                    .map_err(err)?;
                }
                format!("Cor ajustada {}", label(*target, ids.len()))
            }
            Intent::Effect { target, effect, amount } => {
                let ids = resolve(p, ctx, *target, true).map_err(err)?;
                for id in &ids {
                    patch(p, *id, |c, pt| {
                        let mut fx = c.effects.clone();
                        let mut col = c.color.clone();
                        match effect {
                            EffectName::Blur => fx.blur = amount.clamp(1.0, 60.0) as f32,
                            EffectName::Sharpen => fx.sharpen = amount.clamp(0.1, 2.0) as f32,
                            EffectName::Vignette => fx.vignette = amount.clamp(0.1, 1.0) as f32,
                            EffectName::Warm => fx.temperature = 5200.0,
                            EffectName::Cool => fx.temperature = 8000.0,
                            EffectName::BlackWhite => col.saturation = 0.0,
                        }
                        pt.effects = Some(fx);
                        pt.color = Some(col);
                    })
                    .map_err(err)?;
                }
                let name = match effect {
                    EffectName::Blur => "Desfoque",
                    EffectName::Sharpen => "Nitidez",
                    EffectName::Vignette => "Vinheta",
                    EffectName::BlackWhite => "Preto e branco",
                    EffectName::Warm => "Tom quente",
                    EffectName::Cool => "Tom frio",
                };
                format!("{name} {}", label(*target, ids.len()))
            }
            Intent::Transition { kind, seconds, at } => {
                let d = t(seconds.clamp(0.1, 3.0)).snap_to_frame(fps);
                let mut pairs: Vec<(Id, Id, Time)> = vec![];
                for tr in p.timeline.tracks.iter().filter(|t| t.kind == TrackKind::Video && !t.locked) {
                    for w in tr.clips.windows(2) {
                        if w[0].end() == w[1].start
                            && p.timeline.transition_between(w[0].id, w[1].id).is_none()
                            && w[0].duration > d * 2
                            && w[1].duration > d * 2
                        {
                            pairs.push((w[0].id, w[1].id, w[1].start));
                        }
                    }
                }
                if let Some(at) = at {
                    let at_t = t(*at);
                    pairs.sort_by_key(|x| (x.2 - at_t).abs());
                    pairs.truncate(1);
                }
                if pairs.is_empty() {
                    return Err(err("não há cortes entre clipes encostados para receber transição".into()));
                }
                for (a, b, _) in &pairs {
                    ap(p, EditCommand::AddTransition { from_clip: *a, to_clip: *b, kind: *kind, duration: d }).map_err(err)?;
                }
                format!("{} transição(ões) de {:.1} s", pairs.len(), d.as_secs_f64())
            }
            Intent::Marker { at, label: l } => {
                check_t(*at).map_err(err)?;
                ap(p, EditCommand::AddMarker { at: t(*at).snap_to_frame(fps), label: l.chars().take(80).collect() }).map_err(err)?;
                format!("Marcador \"{l}\" em {at:.2} s")
            }
            Intent::Freeze { at, seconds } => {
                check_t(*at).map_err(err)?;
                let at_t = t(*at).snap_to_frame(fps);
                let c = p
                    .timeline
                    .tracks
                    .iter()
                    .filter(|t| t.kind == TrackKind::Video && !t.locked)
                    .flat_map(|t| t.clips.iter())
                    .find(|c| c.range().contains(at_t) && c.asset_id().is_some())
                    .map(|c| c.id)
                    .ok_or_else(|| err(format!("não há vídeo em {at:.2} s")))?;
                let d = t(seconds.clamp(0.2, 30.0)).snap_to_frame(fps);
                ap(p, EditCommand::FreezeFrame { clip_id: c, at: at_t, duration: d }).map_err(err)?;
                format!("Imagem congelada em {:.2} s por {:.1} s", at_t.as_secs_f64(), d.as_secs_f64())
            }
            Intent::Reverse { target } => {
                let ids = resolve(p, ctx, *target, true).map_err(err)?;
                for id in &ids {
                    patch(p, *id, |c, pt| pt.reverse = Some(!c.reverse)).map_err(err)?;
                }
                format!("Reproduzido de trás para frente {}", label(*target, ids.len()))
            }
        };
        done.push(AppliedIntent { intent: i.clone(), summary: pt_decimals(&summary) });
    }
    Ok(done)
}
