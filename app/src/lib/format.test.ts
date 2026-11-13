import { describe, expect, it } from "vitest";
import { bytes, compactCount, duration, exactCount, plural, shortId, shortPath } from "./format";

describe("compactCount", () => {
  it("keeps counts below 1000 exact", () => {
    for (const n of [0, 9, 10, 100, 999]) expect(compactCount(n)).toBe(String(n));
  });

  it("shortens larger counts to at most four characters", () => {
    expect(compactCount(1000)).toBe("1K");
    expect(compactCount(1234)).toBe("1.2K");
    expect(compactCount(9999)).toBe("10K");
    expect(compactCount(10_000)).toBe("10K");
    expect(compactCount(12_345)).toBe("12K");
    expect(compactCount(123_456)).toBe("123K");
    expect(compactCount(999_999)).toBe("1M");
    expect(compactCount(1_234_567)).toBe("1.2M");
  });
});

describe("exactCount", () => {
  it("groups thousands", () => {
    expect(exactCount(7)).toBe("7");
    expect(exactCount(12_345)).toBe("12,345");
  });
});

describe("bytes", () => {
  it("uses whole bytes below 1 KB and one decimal for small values", () => {
    expect(bytes(0)).toBe("0 B");
    expect(bytes(1023)).toBe("1023 B");
    expect(bytes(1536)).toBe("1.5 KB");
    expect(bytes(20_000_000)).toBe("19 MB");
    expect(bytes(5 * 1024 ** 4)).toBe("5.0 TB");
  });
});

describe("duration", () => {
  it("rounds to friendly units", () => {
    expect(duration(5)).toBe("less than a minute left");
    expect(duration(355)).toBe("about 6 min left");
    expect(duration(3 * 3600 + 120)).toBe("about 3 h 2 min left");
  });
});

describe("plural", () => {
  it("picks the right form", () => {
    expect(plural(1, "file")).toBe("1 file");
    expect(plural(2, "file")).toBe("2 files");
    expect(plural(2, "person", "people")).toBe("2 people");
  });
});

describe("shortPath", () => {
  it("keeps short paths as they are", () => {
    expect(shortPath("C:\\Users\\alex")).toBe("C:\\Users\\alex");
  });

  it("keeps the root and the last folders of long Windows paths", () => {
    const p = "C:\\Users\\alex\\AppData\\Local\\Temp\\deep\\nested\\Photos";
    const s = shortPath(p, 30);
    expect(s.startsWith("C:\\…\\")).toBe(true);
    expect(s.endsWith("nested\\Photos")).toBe(true);
    expect(s.length).toBeLessThanOrEqual(34);
  });

  it("handles Unix paths and a single very long segment", () => {
    const long = "x".repeat(60);
    expect(shortPath(`/home/example/${long}`, 20)).toBe(`/…/${long}`);
  });
});

describe("shortId", () => {
  it("shows the ends of long ids", () => {
    expect(shortId("a9cf000000000000b9b4")).toBe("a9cf…b9b4");
    expect(shortId("short")).toBe("short");
  });
});
