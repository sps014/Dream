import { isNode } from "../platform.js";

export function makeDatetimeTextHost() {
  return {
    unicodeNormalize: (text, form) => {
      const forms = ["NFC", "NFD", "NFKC", "NFKD"];
      const f = forms[form] || "NFC";
      return String(text).normalize(f);
    },
    unicodeToLower: (text) => String(text).toLocaleLowerCase(),
    unicodeGraphemes: (text) => {
      const s = String(text);
      if (typeof Intl !== "undefined" && typeof Intl.Segmenter === "function") {
        const seg = new Intl.Segmenter(undefined, { granularity: "grapheme" });
        return Array.from(seg.segment(s), (part) => part.segment);
      }
      return Array.from(s);
    },
    dateNowMillis: () => BigInt(Date.now()),
    dateLocalOffsetMinutes: (millis) => -new Date(Number(millis)).getTimezoneOffset(),
    timeNowNanos: () => {
      if (isNode) {
        return process.hrtime.bigint();
      } else {
        return BigInt(Math.floor(performance.now() * 1000000));
      }
    },
    delayMs: (ms) => new Promise((resolve) => setTimeout(resolve, Math.max(0, ms | 0))),
  };
}
