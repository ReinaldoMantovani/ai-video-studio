// Testes de ponta a ponta da interface (tauri-driver + WebKitWebDriver).
//
// Uso:  node e2e/run.mjs [--app target/release/ai-video-studio] [--keep]
// Requer: tauri-driver no PATH, WebKitWebDriver (Linux: webkit2gtk-driver),
// FFmpeg, um display (Xvfb no CI) e o app compilado com `--features custom-protocol`.
//
// A biblioteca de teste é temporária e preparada pela CLI `avs` (o diálogo de
// arquivos do sistema não é automatizável via WebDriver). Cada cenário roda a
// interface real: cliques, teclado, jobs, renderização e exportação.

import { execFileSync, spawn } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { Driver, Keys, sleep } from "./webdriver.mjs";

const root = path.resolve(path.dirname(new URL(import.meta.url).pathname), "..");
const arg = (k, d) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : d;
};
const APP = path.resolve(arg("--app", path.join(root, "target/release/ai-video-studio")));
const AVS = path.resolve(arg("--avs", path.join(root, "target/release/avs")));
const KEEP = process.argv.includes("--keep");
const ONLY = arg("--only", null);

const work = fs.mkdtempSync(path.join(os.tmpdir(), "avs-e2e-"));
const lib = path.join(work, "lib");
const art = path.join(root, "e2e", "artifacts");
fs.mkdirSync(art, { recursive: true });
// --installed: testa o app instalado pelo pacote, encontrando presets e o worker
// Python nos próprios recursos (sem as variáveis do ambiente de desenvolvimento).
const INSTALLED = process.argv.includes("--installed");
const env = { ...process.env, AVS_LIBRARY: lib };
if (!INSTALLED) Object.assign(env, { AVS_PRESETS_DIR: path.join(root, "presets"), AVS_WORKER_DIR: path.join(root, "workers/python") });
// A CLI de preparação sempre usa os presets do repositório.
const cliEnv = { ...env, AVS_PRESETS_DIR: INSTALLED && fs.existsSync("/usr/lib/AI Video Studio/presets") ? "/usr/lib/AI Video Studio/presets" : path.join(root, "presets") };

function avs(...a) {
  return execFileSync(AVS, ["--library", lib, ...a], { env: cliEnv, encoding: "utf8" });
}
function ffmpeg(...a) {
  execFileSync("ffmpeg", ["-y", "-v", "error", ...a]);
}

// ── Mídia e projeto de teste ──
function fixtures() {
  const media = path.join(work, "e2e.mp4");
  // 12 s: tom com pausas (fala por energia sem Python): 0–3 tom, 3–5 silêncio, 5–8 tom, 8–10 silêncio, 10–12 tom.
  ffmpeg(
    "-f", "lavfi", "-i", "testsrc2=s=640x360:r=30:d=12",
    "-f", "lavfi", "-i", "sine=f=300:d=12:r=48000",
    "-filter_complex", "[1:a]volume='if(between(t,3,5)+between(t,8,10),0,1)':eval=frame[a]",
    "-map", "0:v", "-map", "[a]", "-c:v", "libx264", "-preset", "ultrafast", "-c:a", "aac", "-shortest", media,
  );
  avs("new", "E2E Edição", "--preset", "entrevistas");
  const imp = avs("import", "E2E Edição", media);
  const assetId = imp.match(/\(([0-9a-f-]{36})/)[1];
  const show = avs("show", "E2E Edição");
  const vt = show.match(/\[Video\][^(]*\(([0-9a-f-]{36})\)/)[1];
  avs("edit", "E2E Edição", JSON.stringify({ op: "insert_clip", track_id: vt, asset_id: assetId, start: 0, source_in: null, duration: null, ripple: false }));
  avs("save", "E2E Edição");
  // Projeto independente para o cenário de pausas (a mesma mídia: análise em cache).
  avs("new", "E2E Pausas", "--preset", "entrevistas");
  const imp2 = avs("import", "E2E Pausas", media);
  const a2 = imp2.match(/\(([0-9a-f-]{36})/)[1];
  const vt2 = avs("show", "E2E Pausas").match(/\[Video\][^(]*\(([0-9a-f-]{36})\)/)[1];
  avs("edit", "E2E Pausas", JSON.stringify({ op: "insert_clip", track_id: vt2, asset_id: a2, start: 0, source_in: null, duration: null, ripple: false }));
  avs("save", "E2E Pausas");
  // Projeto longo (5 min) para o preview por janelas.
  const longo = path.join(work, "longo.mp4");
  ffmpeg("-f", "lavfi", "-i", "testsrc2=s=320x180:r=30:d=300", "-f", "lavfi", "-i", "sine=f=220:d=300:r=48000", "-c:v", "libx264", "-preset", "ultrafast", "-c:a", "aac", "-shortest", longo);
  avs("new", "E2E Longo", "--preset", "entrevistas");
  const a3 = avs("import", "E2E Longo", longo).match(/\(([0-9a-f-]{36})/)[1];
  const vt3 = avs("show", "E2E Longo").match(/\[Video\][^(]*\(([0-9a-f-]{36})\)/)[1];
  avs("edit", "E2E Longo", JSON.stringify({ op: "insert_clip", track_id: vt3, asset_id: a3, start: 0, source_in: null, duration: null, ripple: false }));
  avs("save", "E2E Longo");
  return { media };
}

// ── Infra ──
let driverProc;
async function startDriver() {
  driverProc = spawn("tauri-driver", ["--port", "4444"], { env, stdio: ["ignore", "pipe", "pipe"] });
  driverProc.stderr.on("data", (d) => fs.appendFileSync(path.join(art, "tauri-driver.log"), d));
  for (let i = 0; i < 50; i++) {
    try {
      await fetch("http://127.0.0.1:4444/status");
      return;
    } catch {
      await sleep(100);
    }
  }
  throw new Error("tauri-driver não iniciou");
}

const d = new Driver();
async function launch() {
  await d.start(APP);
  await d.waitFor(async () => (await d.bodyText()).includes("Novo projeto"), { what: "dashboard", timeout: 30000 });
}
async function relaunch() {
  await d.quit();
  await sleep(800);
  await launch();
}

const timecode = async () => {
  const m = (await d.bodyText()).match(/(\d\d):(\d\d):(\d\d):(\d\d) \/ \d\d:\d\d:\d\d:\d\d/);
  return m ? +m[1] * 3600 + +m[2] * 60 + +m[3] + +m[4] / 30 : null;
};
/** Leva o cursor ao instante `t` (s) clicando na régua com o zoom ajustado. */
async function seekTo(t) {
  await d.click(await d.find('[aria-label="Ajustar à janela"]'));
  await sleep(300);
  const ticks = await d.exec(`return [...document.querySelectorAll('span')]
    .filter(s => /^\\d+:\\d\\d$/.test(s.textContent.trim()) && s.parentElement && s.parentElement.style.left)
    .map(s => { const r = s.parentElement.getBoundingClientRect(); const [m, x] = s.textContent.trim().split(':').map(Number); return { t: m * 60 + x, x: r.left, y: r.top + r.height - 6 }; })`);
  const a = ticks[0], b = ticks.find((k) => k.t > a.t);
  if (!a || !b) throw new Error("régua sem marcas");
  const x = a.x + ((t - a.t) * (b.x - a.x)) / (b.t - a.t);
  await d.pointerClickAt(x, a.y);
}
const clipCount = (name) => d.exec(`return document.querySelectorAll('[title^="${name} —"]').length`);
// Duração da timeline (barra da timeline), não a do texto de um plano ("Duração 0:12 → 0:10").
const durationLabel = () => d.exec(`return [...document.querySelectorAll('span')].map(e => e.textContent.trim().match(/^Duração (\\d+:\\d\\d)$/)).find(Boolean)?.[1] ?? null`);
async function openProject(name) {
  // No editor? Volta ao dashboard primeiro.
  const back = await d.findAll('[aria-label="Voltar ao início"]');
  if (back.length) {
    await d.click(back[0]);
    await d.waitFor(async () => (await d.bodyText()).includes("Projetos recentes"), { what: "dashboard" });
  }
  await d.click(await d.find(`//*[normalize-space(text())=${JSON.stringify(name)}]`, { xpath: true }));
  await d.waitFor(async () => (await d.findAll('[aria-label="Voltar ao início"]')).length > 0 && (await d.bodyText()).includes("Duração"), { what: `editor de ${name}` });
}
async function tab(label) {
  await d.click(await d.find(`[role="tab"][aria-label="${label}"]`));
  await sleep(300);
}
async function ask(text) {
  const box = await d.find('textarea[aria-label="Pedido ao assistente"]');
  await d.type(box, text);
  await d.keys(Keys.ENTER);
  await d.waitFor(async () => {
    const t = await d.bodyText();
    return t.includes("Aplicar") || t.includes("Não entendi") || t.includes("Passo");
  }, { what: `plano para "${text}"` });
}
async function applyPlan() {
  const btns = await d.findAll(`//button[normalize-space(.)="Aplicar"]`, true);
  await d.click(btns[btns.length - 1]);
  await d.waitFor(async () => (await d.bodyText()).includes("Aplicado. Ctrl+Z desfaz."), { what: "aplicação do plano" });
}
async function focusBody() {
  await d.exec("document.activeElement && document.activeElement.blur(); document.body.focus();");
}

// ── Cenários ──
const scenarios = {
  async dashboard() {
    // Projetos e presets chegam por consultas assíncronas logo após a abertura:
    // espera, em vez de ler a tela uma vez só (falhava às vezes no CI).
    const want = ["Novo projeto", "Edição automática", "21 presets de edição", "E2E Edição", "Gerenciador de IA"];
    let missing = want;
    try {
      await d.waitFor(async () => {
        const t = await d.bodyText();
        missing = want.filter((s) => !t.includes(s));
        return missing.length === 0;
      }, { what: "dashboard completo" });
    } catch {
      throw new Error(`dashboard sem ${missing.map((s) => `"${s}"`).join(", ")}`);
    }
  },

  async gerenciador_ia_e_presets() {
    await d.click(await d.containing("Fala, cenas, rostos", "p"));
    await d.waitFor(async () => (await d.bodyText()).includes("Gerenciador de IA") && (await d.bodyText()).match(/Análise de fala|Detecção de rostos/), { what: "Gerenciador de IA" });
    await d.keys(Keys.ESC);
    await d.click(await d.containing("21 presets de edição", "p"));
    await d.waitFor(async () => {
      const t = await d.bodyText();
      return t.includes("Presets de edição") && t.includes("YouTube Shorts") && t.includes("Entrevistas") && t.includes("Cortes de Lives");
    }, { what: "lista de presets" });
    await d.keys(Keys.ESC);
  },

  async criar_projeto_vazio() {
    await d.click(await d.containing("Escolha um preset de formato", "p"));
    const name = await d.find('input[placeholder="Projeto sem título"]');
    await d.type(name, "Criado pelo E2E");
    await d.click(await d.byText("Criar vazio", "button"));
    await d.waitFor(async () => (await d.bodyText()).includes("A timeline está vazia"), { what: "editor vazio" });
    if (!(await d.bodyText()).includes("Criado pelo E2E")) throw new Error("nome do projeto não aparece");
    await d.click(await d.find('[aria-label="Voltar ao início"]'));
    await d.waitFor(async () => (await d.bodyText()).includes("Criado pelo E2E") && (await d.bodyText()).includes("Projetos recentes"), { what: "projeto no dashboard" });
  },

  async assistente_dividir_desfazer_refazer() {
    await openProject("E2E Edição");
    if ((await clipCount("e2e.mp4")) !== 1) throw new Error(`esperava 1 clipe, há ${await clipCount("e2e.mp4")}`);
    await tab("Assistente");
    await ask("divida em 0:05");
    if (!(await d.bodyText()).includes("Dividido em 5,00 s")) throw new Error("plano de divisão não mostrado");
    await applyPlan();
    await d.waitFor(async () => (await clipCount("e2e.mp4")) === 2, { what: "2 clipes após dividir" });
    await focusBody();
    await d.keys(Keys.CTRL, "z");
    await d.waitFor(async () => (await clipCount("e2e.mp4")) === 1, { what: "desfazer (Ctrl+Z)" });
    await d.keys(Keys.CTRL, Keys.SHIFT, "z");
    await d.waitFor(async () => (await clipCount("e2e.mp4")) === 2, { what: "refazer (Ctrl+Shift+Z)" });
  },

  async assistente_cortar_trecho() {
    await openProject("E2E Edição");
    const before = await durationLabel();
    await tab("Assistente");
    await ask("corte de 0:02 a 0:04");
    await applyPlan();
    await d.waitFor(async () => (await durationLabel()) === "0:10", { what: `duração 0:10 (antes ${before})` });
    await ask("faça um bolo de chocolate");
    if (!(await d.bodyText()).includes("Não entendi")) throw new Error("pedido fora do escopo não explicado");
  },

  async selecao_teclado_e_inspector() {
    await openProject("E2E Edição");
    const n = await clipCount("e2e.mp4");
    const clip = await d.find('[title^="e2e.mp4 —"]');
    await d.pointerClick(clip, 0, 6);
    await d.waitFor(async () => (await d.bodyText()).includes("Volume"), { what: "propriedades do clipe no inspector" });
    await focusBody();
    await d.keys("\uE017"); // Delete
    await d.waitFor(async () => (await clipCount("e2e.mp4")) === n - 1, { what: "clipe apagado com Delete" });
    await d.keys(Keys.CTRL, "z");
    await d.waitFor(async () => (await clipCount("e2e.mp4")) === n, { what: "clipe de volta com Ctrl+Z" });
  },

  async versoes_salvas() {
    await openProject("E2E Edição");
    await tab("Histórico");
    await d.type(await d.find('input[placeholder="Ex.: Versão para Shorts"]'), "Antes do teste");
    await d.click(await d.find(`//input[@placeholder="Ex.: Versão para Shorts"]/following-sibling::button`, { xpath: true }));
    await d.waitFor(async () => (await d.bodyText()).includes("Antes do teste"), { what: "versão na lista" });
    const t = await d.bodyText();
    const line = t.split("\n").slice(t.split("\n").indexOf("Antes do teste"), t.split("\n").indexOf("Antes do teste") + 3).join(" ");
    if (!/\d{2}\/\d{2}\/\d{4} \d{2}:\d{2}/.test(line) || /T\d{2}:/.test(line)) throw new Error(`data da versão mal formatada: ${line}`);
    const n = await clipCount("e2e.mp4");
    await tab("Assistente");
    await ask("divida em 0:01");
    await applyPlan();
    await d.waitFor(async () => (await clipCount("e2e.mp4")) === n + 1, { what: "edição após salvar a versão" });
    await tab("Histórico");
    const row = await d.find(`//p[normalize-space(.)="Antes do teste"]/ancestor::div[2]//button[normalize-space(.)="Restaurar"]`, { xpath: true });
    await d.click(row);
    await d.waitFor(async () => (await clipCount("e2e.mp4")) === n, { what: "versão restaurada" });
  },

  async editar_automaticamente_e_revisao() {
    await openProject("E2E Edição");
    await tab("IA");
    await d.click(await d.containing("Pré-visualizar plano", "button"));
    await d.waitFor(async () => (await d.bodyText()).includes("Aplicar edição") || (await d.bodyText()).includes("Analisar e aplicar"), { what: "plano automático", timeout: 30000 });
    // Mídia ainda não analisada (este cenário roda antes da análise): o caminho
    // de quem abre um projeto novo é "Analisar e aplicar", num único job.
    if (!(await d.bodyText()).includes("Analisar e aplicar")) throw new Error("esperava a oferta de analisar e aplicar");
    await d.click(await d.containing("Analisar e aplicar", "button"));
    await d.waitFor(async () => (await d.bodyText()).includes("Revisão de qualidade") && (await d.bodyText()).match(/ponto\(s\)|Nenhum problema/), { what: "revisão de qualidade após o job", timeout: 90000 });
    await d.keys(Keys.ESC);
    await tab("Histórico");
    await d.waitFor(async () => (await d.bodyText()).includes("Original (antes da edição automática)") && (await d.bodyText()).match(/Edição IA v\d/), { what: "versões antes/depois" });
  },

  async texto() {
    await openProject("E2E Edição");
    await tab("Texto");
    await d.click(await d.byText("Título", "button"));
    await d.waitFor(async () => (await d.exec(`return [...document.querySelectorAll('[title]')].filter(e => e.title.startsWith('Título —')).length`)) >= 1, { what: "clipe de texto na timeline" });
  },

  async analise_e_pausas() {
    await openProject("E2E Pausas");
    await tab("IA");
    await d.click(await d.containing("Analisar mídias", "button"));
    await d.waitFor(async () => (await d.bodyText()).match(/\d+ trecho\(s\) de fala/), { what: "análise concluída", timeout: 90000 });
    await d.click(await d.containing("Calcular pausas", "button"));
    await d.waitFor(async () => (await d.findAll(`//button[normalize-space(.)="Aplicar"]`, true)).length > 0, { what: "plano de pausas" });
    const before = await durationLabel();
    const btns = await d.findAll(`//button[normalize-space(.)="Aplicar"]`, true);
    await d.click(btns[0]);
    await d.waitFor(async () => (await durationLabel()) !== before, { what: `duração menor que ${before}` });
    // 2 pausas de 2 s com folga de 0,25 s em volta da fala: 12 s → ~9 s.
    const after = await durationLabel();
    if (!["0:08", "0:09"].includes(after)) throw new Error(`duração após remover pausas: ${after}`);
  },

  async exportar() {
    await openProject("E2E Edição");
    const out = path.join(work, "exports");
    fs.mkdirSync(out, { recursive: true });
    await d.exec(`localStorage.setItem("avs.exportDir", arguments[0])`, out);
    await d.click(await d.byText("Exportar", "button"));
    await d.waitFor(async () => (await d.findAll(`//div[@role="dialog"]//button[normalize-space(.)="Exportar"]`, true)).length > 0, { what: "diálogo de exportação" });
    const name = await d.find('[role="dialog"] input:not([readonly])');
    await d.type(name, "e2e_export");
    await d.click((await d.findAll(`//div[@role="dialog"]//button[normalize-space(.)="Exportar"]`, true))[0]);
    const file = await d.waitFor(() => fs.readdirSync(out).find((f) => f.endsWith(".mp4") && !f.endsWith(".part.mp4")), { what: "arquivo exportado", timeout: 240000 });
    await d.waitFor(async () => (await d.bodyText()).includes("concluído e validado"), { what: "aviso de exportação validada", timeout: 60000 });
    const probe = execFileSync("ffprobe", ["-v", "error", "-show_entries", "stream=codec_type", "-of", "csv=p=0", path.join(out, file)], { encoding: "utf8" });
    if (!probe.includes("video") || !probe.includes("audio")) throw new Error(`arquivo exportado incompleto: ${probe}`);
  },

  async recuperacao_apos_fechar_sem_salvar() {
    await openProject("E2E Edição");
    await tab("Assistente");
    await ask('marque este ponto como "antes de fechar"');
    await applyPlan();
    await relaunch();
    await openProject("E2E Edição");
    await d.waitFor(async () => (await d.bodyText()).includes("recuperadas do salvamento automático"), { what: "aviso de recuperação" });
    const markers = await d.exec(`return [...document.querySelectorAll('[title]')].filter(e => e.title.includes('antes de fechar')).length`);
    if (markers < 1) throw new Error("marcador feito antes de fechar não foi recuperado");
  },

  async reproducao_e_preview_por_janelas() {
    // Reprodução real do preview (no Linux, pelo servidor local de mídia) e troca
    // automática de janela numa timeline longa (> 4 min).
    await openProject("E2E Longo");
    await d.waitFor(async () => (await d.bodyText()).includes("Preview atualizado"), { what: "primeira janela (0:00–2:00)", timeout: 180000 });
    await seekTo(110);
    await d.waitFor(async () => Math.abs(((await timecode()) ?? 0) - 110) < 2, { what: "cursor em ~1:50" });
    await d.waitFor(async () => (await d.bodyText()).includes("Preview atualizado"), { what: "janela atual cobre o cursor" });
    const src0 = await d.exec(`return document.querySelector('video')?.src ?? ''`);
    await d.click(await d.find('[aria-label="Reproduzir (Espaço)"]'));
    await d.waitFor(async () => ((await timecode()) ?? 0) > 112, { what: "reprodução avançando", timeout: 15000 });
    // Passa do fim da janela (2:00) para a próxima, preparada durante a reprodução.
    await d.waitFor(async () => ((await timecode()) ?? 0) > 123, { what: "reprodução atravessando o fim da janela", timeout: 120000 });
    const src1 = await d.exec(`return document.querySelector('video')?.src ?? ''`);
    if (src1 === src0 || !/preview-r\d+-\d+\.mp4/.test(decodeURIComponent(src1))) throw new Error(`a janela não trocou: ${decodeURIComponent(src1)}`);
    await d.click(await d.find('[aria-label="Pausar (Espaço)"]'));
  },

  async cortes_verticais() {
    await openProject("E2E Edição");
    await tab("Cortes");
    await d.waitFor(async () => (await d.bodyText()).includes("Sugerir cortes"), { what: "aba Cortes" });
    const inputs = await d.findAll('input[type="number"]');
    // Qtd., Mín., Alvo, Máx.
    for (const [i, v] of [[0, "1"], [1, "2"], [2, "4"], [3, "8"]]) await d.type(inputs[i], v);
    await d.click(await d.containing("Sugerir cortes", "button"));
    await d.waitFor(async () => (await d.bodyText()).match(/Criar 1 corte/), { what: "sugestão de corte" });
    await d.click(await d.containing("Criar 1 corte", "button"));
    await d.waitFor(async () => (await d.bodyText()).includes("Cortes criados"), { what: "corte criado", timeout: 60000 });
    await d.click(await d.containing("Abrir", "button"));
    await d.waitFor(async () => (await d.bodyText()).includes("1080×1920"), { what: "projeto vertical aberto" });
  },
};

// ── Execução ──
const results = [];
try {
  fixtures();
  await startDriver();
  await launch();
  for (const [name, fn] of Object.entries(scenarios)) {
    if (ONLY && !ONLY.split(",").includes(name)) continue;
    const t0 = Date.now();
    try {
      await fn();
      results.push({ name, ok: true, ms: Date.now() - t0 });
      console.log(`✔ ${name} (${((Date.now() - t0) / 1000).toFixed(1)} s)`);
    } catch (e) {
      results.push({ name, ok: false, ms: Date.now() - t0, error: String(e.message ?? e) });
      console.log(`✘ ${name}: ${e.message ?? e}`);
      await d.screenshot(path.join(art, `${name}.png`)).catch(() => {});
      fs.writeFileSync(path.join(art, `${name}.txt`), String(await d.bodyText().catch(() => "")));
      await relaunch().catch(() => {});
    }
  }
} finally {
  await d.quit();
  driverProc?.kill();
  if (!KEEP) fs.rmSync(work, { recursive: true, force: true });
}
fs.writeFileSync(path.join(art, "results.json"), JSON.stringify(results, null, 2));
const failed = results.filter((r) => !r.ok).length;
console.log(`\n${results.length - failed}/${results.length} cenários passaram`);
process.exit(failed ? 1 : 0);
