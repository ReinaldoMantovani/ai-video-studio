import { describe, expect, it } from "vitest";
import { FLICKS, interpolate, frameOf, fromFrames, shortDuration, snap, snapToFrame, timecode } from "./time";

const f30 = { num: 30, den: 1 };
const ntsc = { num: 30000, den: 1001 };

describe("tempo em flicks", () => {
  it("converte frames de forma exata (igual ao Rust)", () => {
    expect(fromFrames(30, f30)).toBe(FLICKS);
    expect(fromFrames(30000, ntsc)).toBe(1001 * FLICKS);
    expect(frameOf(fromFrames(1234, ntsc), ntsc)).toBe(1234);
  });

  it("formata timecode por frame", () => {
    expect(timecode(fromFrames(30 * 75 + 12, f30), f30)).toBe("00:01:15:12");
    expect(timecode(0, f30)).toBe("00:00:00:00");
  });

  it("arredonda para a borda de frame", () => {
    expect(snapToFrame(FLICKS / 30 + 10, f30)).toBe(FLICKS / 30);
  });

  it("durações curtas", () => {
    expect(shortDuration(65 * FLICKS)).toBe("1:05");
    expect(shortDuration(3723 * FLICKS)).toBe("1:02:03");
  });

  it("snap magnético usa o alvo mais próximo dentro da tolerância", () => {
    expect(snap(100, [0, 95, 103], 10)).toEqual({ t: 103, target: 103 });
    expect(snap(100, [0, 200], 10)).toEqual({ t: 100, target: null });
  });

  it("interpola keyframes como o núcleo", () => {
    const k = [
      { at: 0, value: 1, easing: "linear" as const },
      { at: 10, value: 3, easing: "ease_in_out" as const },
      { at: 20, value: 5, easing: "linear" as const },
    ];
    expect(interpolate(k, 5, 0)).toBe(2);
    expect(interpolate(k, 15, 0)).toBe(4);
    expect(interpolate(k, 12, 0)).toBeCloseTo(3 + 2 * (0.2 * 0.2 * (3 - 0.4)));
    expect(interpolate([], 3, 7)).toBe(7);
    expect(interpolate(k, 99, 0)).toBe(5);
  });
});
