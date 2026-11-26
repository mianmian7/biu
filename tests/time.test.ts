import moment from "moment";
import momentDurationFormatSetup from "moment-duration-format";
import { describe, expect, test } from "vitest";

import {
  addSecondsUnix,
  daysAgoStartMs,
  endOfDayMs,
  formatDateTime,
  formatDuration,
  formatMillisecond,
  formatRelativeTimeZh,
  formatSecondsToDate,
  nowUnix,
  startOfDayMs,
} from "@/common/utils/time";

import "moment/locale/zh-cn";

// moment 只在测试中作为对照基准，运行时代码已不再引用
momentDurationFormatSetup(moment as any);

const momentDuration = (seconds: number) => {
  const dur = moment.duration(seconds, "seconds") as any;
  return seconds >= 3600 ? dur.format("hh:mm:ss", { trim: false }) : dur.format("mm:ss", { trim: false });
};

describe("time utils match the previous moment implementation", () => {
  test("formatDuration", () => {
    const samples = [
      0, 0.4, 0.5, 0.6, 1, 1.5, 2.49, 2.5, 9, 59, 59.5, 59.99, 60, 61, 599.6, 600, 3599, 3599.4, 3599.5, 3600, 3661.7,
      7322, 86399, 86400, 90061, 360000, -5, -65,
    ];
    for (const s of samples) {
      expect(formatDuration(s), `seconds=${s}`).toBe(momentDuration(s));
    }
    expect(formatDuration(Number.NaN)).toBe("00:00");
  });

  test("formatSecondsToDate / formatMillisecond / formatDateTime", () => {
    const seconds = [1, 86399, 1_600_000_000, 1_700_000_123, 1_735_660_800, 1_767_196_799];
    for (const s of seconds) {
      expect(formatSecondsToDate(s)).toBe(moment.unix(s).format("YYYY-MM-DD"));
      expect(formatMillisecond(s * 1000 + 999)).toBe(moment(s * 1000 + 999).format("YYYY-MM-DD"));
      expect(formatDateTime(s)).toBe(moment.unix(s).format("YYYY-MM-DD HH:mm"));
    }
    expect(formatSecondsToDate(undefined)).toBe("");
    expect(formatSecondsToDate(0)).toBe("");
    expect(formatMillisecond(undefined)).toBe("");
    expect(formatMillisecond(0)).toBe("");
    expect(formatDateTime(undefined)).toBe("");
  });

  test("formatRelativeTimeZh matches moment zh-cn fromNow", () => {
    const now = new Date(2026, 5, 15, 12, 0, 0).getTime();
    const secondsAgo = [
      0,
      10,
      44,
      45,
      89,
      90,
      150,
      60 * 44,
      60 * 44 + 29,
      60 * 44 + 31,
      60 * 45,
      60 * 89,
      60 * 90,
      3600 * 21,
      3600 * 21.6,
      3600 * 22,
      3600 * 35,
      3600 * 36,
      86400 * 25,
      86400 * 25.6,
      86400 * 26,
      86400 * 45,
      86400 * 46,
      86400 * 319,
      86400 * 320,
      86400 * 547,
      86400 * 548,
      86400 * 800,
      86400 * 3650,
      -10,
      -100,
      -86400 * 3,
    ];
    for (const s of secondsAgo) {
      const ms = now - s * 1000;
      expect(formatRelativeTimeZh(ms, now), `secondsAgo=${s}`).toBe(moment(ms).locale("zh-cn").from(moment(now)));
    }
    expect(formatRelativeTimeZh(Number.NaN, now)).toBe("");
  });

  test("formatRelativeTimeZh matches moment across month-end / leap-year / random dates", () => {
    const bases = [
      new Date(2024, 1, 29, 23, 59).getTime(),
      new Date(2025, 0, 31, 0, 0, 1).getTime(),
      new Date(2026, 2, 31, 12).getTime(),
      new Date(2026, 9, 26, 1, 30).getTime(),
    ];
    let seed = 42;
    const rand = () => {
      seed = (seed * 1103515245 + 12345) % 2 ** 31;
      return seed / 2 ** 31;
    };
    for (const now of bases) {
      for (let i = 0; i < 400; i++) {
        // 覆盖几秒到约 6 年的过去与未来
        const offset = Math.round(Math.exp(rand() * Math.log(86400 * 365 * 6)) * 1000) * (rand() < 0.15 ? -1 : 1);
        const ms = now - offset;
        expect(formatRelativeTimeZh(ms, now), `now=${now} ms=${ms}`).toBe(moment(ms).locale("zh-cn").from(moment(now)));
      }
    }
  });

  test("unix helpers", () => {
    const now = 1_700_000_123_456;
    expect(nowUnix(now)).toBe(moment(now).unix());
    expect(addSecondsUnix(30, now)).toBe(moment(now).add(30, "seconds").unix());
    expect(addSecondsUnix(2 * 24 * 60 * 60, now)).toBe(
      moment(now)
        .add(2 * 24 * 60 * 60, "seconds")
        .unix(),
    );
    expect(Math.abs(nowUnix() - moment().unix())).toBeLessThanOrEqual(1);
  });

  test("day range helpers", () => {
    const now = new Date(2026, 2, 1, 8, 30).getTime(); // 跨月：减 1 天回到 2 月
    expect(startOfDayMs(now)).toBe(moment(now).startOf("day").valueOf());
    expect(endOfDayMs(now)).toBe(moment(now).endOf("day").valueOf());
    expect(daysAgoStartMs(1, now)).toBe(moment(now).subtract(1, "days").startOf("day").valueOf());
    expect(endOfDayMs(daysAgoStartMs(1, now))).toBe(moment(now).subtract(1, "days").endOf("day").valueOf());
    expect(daysAgoStartMs(7, now)).toBe(moment(now).subtract(7, "days").startOf("day").valueOf());
  });
});
