import { describe, expect, it } from "vitest";
import { snapshotDate } from "./LeftPanel";

describe("data das versões salvas", () => {
  it("formata RFC 3339 no fuso local, sem o texto ISO cru", () => {
    const s = snapshotDate("2026-11-13T21:38:50.123+00:00");
    expect(s).toMatch(/^\d{2}\/11\/2026 \d{2}:\d{2}$/);
    expect(s).not.toContain("T");
  });
  it("ainda entende o formato antigo do nome do arquivo", () => {
    expect(snapshotDate("20261113-213850123")).toMatch(/^\d{2}\/11\/2026 \d{2}:\d{2}$/);
  });
  it("valor inválido aparece como veio", () => {
    expect(snapshotDate("??")).toBe("??");
  });
});
