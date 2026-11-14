// Cliente WebDriver mínimo (W3C) para o tauri-driver — sem dependências.
const ELEMENT = "element-6066-11e4-a52e-4f735466cecf";
export const Keys = { CTRL: "", SHIFT: "", ENTER: "", ESC: "", BACKSPACE: "" };

export class Driver {
  constructor(url = "http://127.0.0.1:4444") {
    this.url = url;
    this.sid = null;
  }
  async req(method, path, body) {
    const r = await fetch(this.url + path, { method, headers: { "content-type": "application/json" }, body: body ? JSON.stringify(body) : undefined });
    const j = await r.json().catch(() => ({}));
    if (!r.ok || j.value?.error) throw new Error(`${method} ${path}: ${j.value?.error ?? r.status} ${j.value?.message ?? ""}`);
    return j.value;
  }
  s(path) {
    return `/session/${this.sid}${path}`;
  }
  async start(application) {
    const v = await this.req("POST", "/session", { capabilities: { alwaysMatch: { "tauri:options": { application } } } });
    this.sid = v.sessionId;
  }
  async quit() {
    if (this.sid) await this.req("DELETE", this.s("")).catch(() => {});
    this.sid = null;
  }
  async exec(script, ...args) {
    return this.req("POST", this.s("/execute/sync"), { script, args });
  }
  async find(sel, { timeout = 10000, xpath = false } = {}) {
    const t0 = Date.now();
    let last;
    while (Date.now() - t0 < timeout) {
      try {
        const v = await this.req("POST", this.s("/element"), { using: xpath ? "xpath" : "css selector", value: sel });
        return v[ELEMENT];
      } catch (e) {
        last = e;
        await sleep(150);
      }
    }
    throw new Error(`elemento não encontrado: ${sel} (${last?.message})`);
  }
  async findAll(sel, xpath = false) {
    const v = await this.req("POST", this.s("/elements"), { using: xpath ? "xpath" : "css selector", value: sel });
    return v.map((e) => e[ELEMENT]);
  }
  /** Botão (ou elemento) pelo texto visível. */
  async byText(text, tag = "*", opts = {}) {
    return this.find(`//${tag}[normalize-space(.)=${xq(text)} or @aria-label=${xq(text)} or @title=${xq(text)}]`, { ...opts, xpath: true });
  }
  async containing(text, tag = "*", opts = {}) {
    return this.find(`//${tag}[contains(normalize-space(.), ${xq(text)})]`, { ...opts, xpath: true });
  }
  async click(el) {
    // Clique via JS: estável com animações/sobreposições do layout.
    await this.exec("arguments[0].scrollIntoView({block:'center'}); arguments[0].click();", { [ELEMENT]: el });
  }
  /** Clique real do ponteiro (dispara pointerdown/up, como o mouse). */
  async pointerClick(el, dx = 0, dy = 0) {
    await this.exec("arguments[0].scrollIntoView({block:'center'})", { [ELEMENT]: el });
    await this.req("POST", this.s("/actions"), {
      actions: [{ type: "pointer", id: "mouse", parameters: { pointerType: "mouse" }, actions: [
        { type: "pointerMove", duration: 0, origin: { [ELEMENT]: el }, x: dx, y: dy },
        { type: "pointerDown", button: 0 },
        { type: "pointerUp", button: 0 },
      ] }],
    });
    await this.req("DELETE", this.s("/actions")).catch(() => {});
  }
  /** Clique do ponteiro em coordenadas da janela. */
  async pointerClickAt(x, y) {
    await this.req("POST", this.s("/actions"), {
      actions: [{ type: "pointer", id: "mouse", parameters: { pointerType: "mouse" }, actions: [
        { type: "pointerMove", duration: 0, origin: "viewport", x: Math.round(x), y: Math.round(y) },
        { type: "pointerDown", button: 0 },
        { type: "pointerUp", button: 0 },
      ] }],
    });
    await this.req("DELETE", this.s("/actions")).catch(() => {});
  }
  async text(el) {
    return this.req("GET", this.s(`/element/${el}/text`));
  }
  async type(el, text) {
    await this.req("POST", this.s(`/element/${el}/clear`), {}).catch(() => {});
    await this.req("POST", this.s(`/element/${el}/value`), { text });
  }
  async keys(...seq) {
    // seq: lista de teclas; modificadores ficam pressionados até o fim.
    const mods = seq.filter((k) => k === Keys.CTRL || k === Keys.SHIFT);
    const rest = seq.filter((k) => !mods.includes(k));
    const actions = [...mods.map((k) => ({ type: "keyDown", value: k })), ...rest.flatMap((k) => [{ type: "keyDown", value: k }, { type: "keyUp", value: k }]), ...mods.reverse().map((k) => ({ type: "keyUp", value: k }))];
    await this.req("POST", this.s("/actions"), { actions: [{ type: "key", id: "kb", actions }] });
    await this.req("DELETE", this.s("/actions")).catch(() => {});
  }
  async waitFor(fn, { timeout = 20000, what = "condição" } = {}) {
    const t0 = Date.now();
    let last;
    while (Date.now() - t0 < timeout) {
      try {
        const v = await fn();
        if (v) return v;
      } catch (e) {
        last = e;
      }
      await sleep(200);
    }
    throw new Error(`tempo esgotado esperando ${what}${last ? `: ${last.message}` : ""}`);
  }
  async bodyText() {
    return this.exec("return document.body.innerText");
  }
  async screenshot(path) {
    const b64 = await this.req("GET", this.s("/screenshot"));
    (await import("node:fs")).writeFileSync(path, Buffer.from(b64, "base64"));
  }
}

export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
function xq(s) {
  return s.includes('"') ? `concat("${s.split('"').join(`", '"', "`)}")` : `"${s}"`;
}
