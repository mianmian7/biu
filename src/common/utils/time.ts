/**
 * 零依赖的时间工具（替代 moment / moment-duration-format）。
 * 输出格式与此前 moment 实现保持一致，tests/time.test.ts 用 moment 做了逐项对照。
 */

const pad2 = (n: number) => String(n).padStart(2, "0");

/** 秒数 → `mm:ss`（不足 1 小时）或 `hh:mm:ss`；小时数不按天取模 */
export function formatDuration(seconds: number) {
  if (!Number.isFinite(seconds)) return "00:00";
  const sign = seconds < 0 ? "-" : "";
  const total = Math.round(Math.abs(seconds));
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;

  if (seconds >= 3600) {
    return `${sign}${pad2(h)}:${pad2(m)}:${pad2(s)}`;
  }
  // 与 moment-duration-format 一致：3599.5 秒四舍五入后显示 60:00
  return `${sign}${pad2(h * 60 + m)}:${pad2(s)}`;
}

const formatDate = (date: Date) => `${date.getFullYear()}-${pad2(date.getMonth() + 1)}-${pad2(date.getDate())}`;

const isValidDate = (date: Date) => !Number.isNaN(date.getTime());

/** Unix 秒 → `YYYY-MM-DD` */
export const formatSecondsToDate = (s?: number) => {
  if (!s) return "";
  const date = new Date(s * 1000);
  return isValidDate(date) ? formatDate(date) : "";
};

/** 毫秒时间戳 → `YYYY-MM-DD` */
export const formatMillisecond = (s?: number) => {
  if (!s) return "";
  const date = new Date(s);
  return isValidDate(date) ? formatDate(date) : "";
};

/** Unix 秒 → `YYYY-MM-DD HH:mm` */
export const formatDateTime = (s?: number) => {
  if (s === undefined || s === null) return "";
  const date = new Date(s * 1000);
  if (!isValidDate(date)) return "";
  return `${formatDate(date)} ${pad2(date.getHours())}:${pad2(date.getMinutes())}`;
};

/** 本地时间加 n 个月，日期超出目标月天数时取月末（与 moment `add(n, "M")` 一致） */
const addMonths = (ms: number, n: number) => {
  const d = new Date(ms);
  const day = d.getDate();
  d.setDate(1);
  d.setMonth(d.getMonth() + n);
  const lastDay = new Date(d.getFullYear(), d.getMonth() + 1, 0).getDate();
  d.setDate(Math.min(day, lastDay));
  return d.getTime();
};

/** moment 的 positiveMomentsDifference：整月数（日历月） + 剩余毫秒，要求 base <= other */
const monthsAndRemainder = (base: number, other: number) => {
  const b = new Date(base);
  const o = new Date(other);
  let months = o.getMonth() - b.getMonth() + (o.getFullYear() - b.getFullYear()) * 12;
  if (addMonths(base, months) > other) months -= 1;
  return { months, milliseconds: other - addMonths(base, months) };
};

const DAYS_PER_400_YEARS = 146097;
const MONTHS_PER_400_YEARS = 4800;

/**
 * 与 `moment(ms).locale("zh-cn").fromNow()` 相同的相对时间（默认阈值、日历月差）。
 * 例：几秒前、1 分钟前、3 小时前、1 天前、2 个月前、1 年前、2 分钟后。
 */
export function formatRelativeTimeZh(ms: number, now: number = Date.now()) {
  if (!Number.isFinite(ms) || !Number.isFinite(now)) return "";
  const isFuture = ms > now;
  const { months: monthDiff, milliseconds } = isFuture ? monthsAndRemainder(now, ms) : monthsAndRemainder(ms, now);

  // 对应 moment Duration#as：秒/分/时/天先把整月折算成（取整的）天数，月/年把余下毫秒折算成月
  const monthDays = Math.round((monthDiff * DAYS_PER_400_YEARS) / MONTHS_PER_400_YEARS);
  const seconds = Math.round(monthDays * 86400 + milliseconds / 1000);
  const minutes = Math.round(monthDays * 1440 + milliseconds / 60_000);
  const hours = Math.round(monthDays * 24 + milliseconds / 3_600_000);
  const days = Math.round(monthDays + milliseconds / 86_400_000);
  const monthsExact = monthDiff + ((milliseconds / 86_400_000) * MONTHS_PER_400_YEARS) / DAYS_PER_400_YEARS;
  const months = Math.round(monthsExact);
  const years = Math.round(monthsExact / 12);

  let text: string;
  if (seconds < 45) text = "几秒";
  else if (minutes <= 1) text = "1 分钟";
  else if (minutes < 45) text = `${minutes} 分钟`;
  else if (hours <= 1) text = "1 小时";
  else if (hours < 22) text = `${hours} 小时`;
  else if (days <= 1) text = "1 天";
  else if (days < 26) text = `${days} 天`;
  else if (months <= 1) text = "1 个月";
  else if (months < 11) text = `${months} 个月`;
  else if (years <= 1) text = "1 年";
  else text = `${years} 年`;

  return isFuture ? `${text}后` : `${text}前`;
}

/** 当前 Unix 秒（等同 `moment().unix()`） */
export const nowUnix = (now: number = Date.now()) => Math.floor(now / 1000);

/** 当前时间加若干秒后的 Unix 秒 */
export const addSecondsUnix = (seconds: number, now: number = Date.now()) => nowUnix(now + seconds * 1000);

/** 当天 00:00:00.000 的毫秒时间戳（本地时区） */
export const startOfDayMs = (ms: number = Date.now()) => {
  const d = new Date(ms);
  d.setHours(0, 0, 0, 0);
  return d.getTime();
};

/** 当天 23:59:59.999 的毫秒时间戳（本地时区） */
export const endOfDayMs = (ms: number = Date.now()) => {
  const d = new Date(ms);
  d.setHours(23, 59, 59, 999);
  return d.getTime();
};

/** N 天前（按日历日，本地时区）当天 00:00 的毫秒时间戳 */
export const daysAgoStartMs = (days: number, ms: number = Date.now()) => {
  const d = new Date(ms);
  d.setDate(d.getDate() - days);
  d.setHours(0, 0, 0, 0);
  return d.getTime();
};
