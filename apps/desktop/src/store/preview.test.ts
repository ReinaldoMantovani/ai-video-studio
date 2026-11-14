import { beforeEach, describe, expect, it } from "vitest";
import { FLICKS } from "../lib/time";
import { useApp } from "./app";

const s = (x: number) => x * FLICKS;
const win = (a: number, b: number, rev = 3) => ({ path: `/r/preview-r${rev}-${a}.mp4`, revision: rev, range: { start: s(a), end: s(b) } });

describe("preview por janelas", () => {
  beforeEach(() => useApp.setState({ preview: null, nextPreview: null, playhead: 0 }));

  it("primeira janela vira o preview atual", () => {
    useApp.getState().previewReady(win(0, 120));
    expect(useApp.getState().preview?.range?.end).toBe(s(120));
    expect(useApp.getState().nextPreview).toBeNull();
  });

  it("janela seguinte, pronta enquanto a atual ainda cobre o cursor, fica como próxima", () => {
    useApp.getState().previewReady(win(0, 120));
    useApp.setState({ playhead: s(100) });
    useApp.getState().previewReady(win(105, 225));
    expect(useApp.getState().preview?.range?.start).toBe(0);
    expect(useApp.getState().nextPreview?.range?.start).toBe(s(105));
  });

  it("depois de um salto do cursor para fora da janela, a nova vira a atual", () => {
    useApp.getState().previewReady(win(0, 120));
    useApp.setState({ playhead: s(600) });
    useApp.getState().previewReady(win(595, 715));
    expect(useApp.getState().preview?.range?.start).toBe(s(595));
    expect(useApp.getState().nextPreview).toBeNull();
  });

  it("nova revisão substitui o preview (mesmo que a janela antiga cubra o cursor)", () => {
    useApp.getState().previewReady(win(0, 120, 3));
    useApp.setState({ playhead: s(10) });
    useApp.getState().previewReady(win(5, 125, 4));
    expect(useApp.getState().preview?.revision).toBe(4);
  });
});
