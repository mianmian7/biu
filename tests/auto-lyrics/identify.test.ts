import { describe, expect, it } from "vitest";

import type { IdentifyInput, SongCandidate } from "../../shared/auto-lyrics/identify";

import {
  clean,
  identifyPrimaryQueries,
  identifyQueries,
  identifyRules,
  nameForms,
  normalize,
  sameName,
  strongMatch,
  titleVariants,
} from "../../shared/auto-lyrics/identify";
import { buildSearchQueries } from "../../shared/auto-lyrics/title-candidates";

const song: SongCandidate = { id: 1, name: "名もなき何もかも", artists: ["トゲナシトゲアリ"], duration: 189 };
const input: IdentifyInput = {
  title: "【官方MV】トゲナシトゲアリ「名もなき何もかも」",
  duration: 189,
  tid: 31,
  ownerName: "搬运工",
};

describe("rule identification", () => {
  it("cleans display noise and preserves Japanese bracketed titles", () => {
    expect(clean("【Official MV】名もなき何もかも BV1Vb421q7ro P2 4K")).toBe("名もなき何もかも");
    expect(normalize("ＡＢＣ ß １２")).toBe("abcss12");
    expect(identifyQueries(input)).toContain("トゲナシトゲアリ 名もなき何もかも");
  });

  it("adds quoted short names from PR #263 title candidates as fallback queries", () => {
    const title = "中日双语版《雨爱》老头子携影分身#合唱";
    const queries = identifyQueries({ ...input, title });
    expect(queries).toContain("雨爱");
    const rainLove = { id: 9, name: "雨爱", artists: ["杨丞琳"], duration: 260 };
    expect(identifyRules({ ...input, title, duration: 262 }, [rainLove])).toMatchObject({ status: "matched" });
  });

  it("accepts MV, Live and TV-size video titles against an original studio candidate", () => {
    for (const suffix of ["MV", "Live 现场", "TV size"]) {
      expect(identifyRules({ ...input, title: `${song.name} ${suffix}` }, [song])).toMatchObject({
        status: "matched",
        song,
      });
    }
    expect(identifyRules({ ...input, tid: 17 }, [song]).status).toBe("matched");
    expect(identifyRules({ ...input, tid: 1003, title: song.name }, [song]).status).toBe("matched");
    expect(identifyRules({ ...input, tid: 17, title: "游戏实况" }, [song]).status).toBe("not-music");
  });

  it("rejects collections, tutorials and karaoke before requesting searches", () => {
    for (const title of ["吉他教程", "名もなき何もかも 伴奏", "TAB谱"]) {
      expect(identifyRules({ ...input, title }, [song]).status).toBe("not-music");
      expect(identifyQueries({ ...input, title })).toEqual([]);
    }
    expect(identifyRules({ ...input, title: "全曲合集" }, [song]).status).toBe("collection");
    expect(identifyRules({ ...input, title: "全曲合集", pageTitle: song.name }, [song]).status).toBe("matched");
    expect(identifyRules({ ...input, title: "全曲合集", pageTitle: "正片" }, [song]).status).toBe("collection");
  });

  it("leaves same-title artists to alignment and rejects multiple explicit songs", () => {
    const cover = { ...song, id: 2, artists: ["another artist"] };
    expect(identifyRules({ ...input, title: song.name }, [song, cover])).toMatchObject({
      status: "ambiguous",
      contenders: [song, cover],
    });
    expect(identifyRules(input, [song, cover])).toMatchObject({ status: "matched", song });
    const other = { ...song, id: 3, name: "空の箱" };
    expect(identifyRules({ ...input, title: `${song.name} / 空の箱` }, [song, other]).status).toBe("unidentified");
    expect(identifyRules({ ...input, title: song.name, ownerName: cover.artists[0] }, [song, cover]).status).toBe(
      "ambiguous",
    );
  });

  it("connects artist aliases and chooses the nearest studio duration", () => {
    const variants = [song, { ...song, id: 2, artists: ["トゲナシトゲアリ (Togenashi Togeari)"], duration: 200 }];
    expect(identifyRules({ ...input, title: song.name, duration: 201 }, variants)).toMatchObject({
      status: "matched",
      song: { id: 2 },
    });
    expect(sameName("MyGO!!!!!", "MyGO!!!!! (迷跡波)")).toBe(true);
    expect(nameForms("A / B (C)")).toEqual(new Set(["abc", "a", "bc", "ab", "c", "b"]));
  });

  it("supports aliases and version suffixes while excluding live/remix candidates", () => {
    expect(titleVariants("青空 (TV size)")).toEqual(["青空 (TV size)", "青空"]);
    expect(titleVariants("青空 (別の曲)")).toEqual(["青空 (別の曲)"]);
    const aliased = { ...song, name: "運命の華", aliases: ["命运之花"] };
    expect(identifyRules({ ...input, title: "命运之花 MV" }, [aliased]).status).toBe("matched");
    for (const suffix of ["(Live)", "(remix)", "(instrumental)"]) {
      expect(identifyRules(input, [{ ...song, name: `${song.name} ${suffix}` }]).status).toBe("unidentified");
    }
  });

  it("matches short Japanese names as whole spans and ASCII names at word boundaries", () => {
    expect(strongMatch("花", "「花」Live")).toBe(true);
    expect(strongMatch("花", "花火 MV")).toBe(false);
    expect(strongMatch("art", "cartwheel")).toBe(false);
    expect(strongMatch("Don't stop", "DON'T_STOP MV")).toBe(true);
  });

  // Real Mac E2E titles that NetEase search matched to The Kid LAROI "GIRLS".
  const gbcTitles = {
    BV19T421S7nv:
      '【Girls Band Cry现场高音质】 トゲナシトゲアリ 皆无其名「名もなき何もかも」 1st LIVE "薄明の序奏”  ガールズバンドクライ',
    BV15t421K7Le:
      '【2024年3月16日】トゲナシトゲアリ 1st ONE-MAN LIVE "薄明の序奏” 名もなき何もかも【ガールズバンドクライ】',
    BV1QhieYjEY2: "【GBC】空の箱 - Girls Band Cry トゲナシトゲアリ(无刺有刺) 3rd ONE-MAN LIVE “咆哮の奏” 2024",
    BV1dy75z8Enu: "【GIRLS BAND CRY/LIVE】爆ぜて咲く。现场互动拉满，还有最爱的桃仁饭看！",
  };

  it("does not let a one-word English title match the first word of a longer English name", () => {
    for (const title of Object.values(gbcTitles)) {
      expect(strongMatch("GIRLS", title)).toBe(false);
      expect(strongMatch("girls", title)).toBe(false);
      const laroi = { id: 9, name: "GIRLS", artists: ["The Kid LAROI"], duration: 200 };
      expect(identifyRules({ ...input, title, duration: 200 }, [laroi]).status).toBe("unidentified");
    }
    expect(strongMatch("GIRLS", "The Kid LAROI - GIRLS (Official Video)")).toBe(true);
    expect(strongMatch("GIRLS", "【MV】GIRLS / The Kid LAROI")).toBe(true);
    expect(strongMatch("Lemon", "米津玄師 Lemon MV")).toBe(true);
    expect(strongMatch("Girls Band Cry", gbcTitles.BV1QhieYjEY2)).toBe(true);
  });

  // NetEase search results recorded on the Mac (2026-10-29), trimmed to the fields identify uses.
  const laroiHits = [
    { id: 1, name: "girls", artists: ["The Kid LAROI"], duration: 153 },
    { id: 2, name: "GIRLS", artists: ["The Kid LAROI"], duration: 153 },
  ];
  const namonakiHits = [
    {
      id: 2051267438,
      name: "名もなき何もかも",
      artists: ["トゲナシトゲアリ"],
      duration: 188.784,
      aliases: ["皆无其名（TV动画 Girls Band Cry 第七话插入曲）"],
    },
    {
      id: 2149020724,
      name: "名もなき何もかも",
      artists: ["トゲナシトゲアリ"],
      duration: 189.118,
      aliases: ["皆无其名"],
    },
    { id: 2629442227, name: "名もなき何もかも", artists: ["Hanser"], duration: 189.309 },
    {
      id: 2652041598,
      name: "名もなき何もかも",
      artists: ["トゲナシトゲアリ"],
      duration: 189.118,
      aliases: ["皆无其名（TV动画 Girls Band Cry 第七话插入曲）"],
    },
    { id: 2629306577, name: "名もなき何もかも", artists: ["orenji"], duration: 188.784 },
    { id: 2645905503, name: "皆无其名", artists: ["翅月"], duration: 188.784 },
  ];
  const hazeteHits = [
    { id: 2077537956, name: "爆ぜて咲く", artists: ["トゲナシトゲアリ"], duration: 221.44, aliases: ["爆裂绽放"] },
    { id: 2149019978, name: "爆ぜて咲く", artists: ["トゲナシトゲアリ"], duration: 223.061, aliases: ["爆裂绽放"] },
    { id: 2670185220, name: "爆ぜて咲く", artists: ["闪耀圣爱音", "夢ノ結唱 ROSE"], duration: 221.44 },
    { id: 2752741984, name: "爆ぜて咲く", artists: ["晓月亨"], duration: 221.16 },
    { id: 2652041600, name: "爆ぜて咲く", artists: ["トゲナシトゲアリ"], duration: 221.44, aliases: ["爆裂绽放"] },
  ];

  it("treats a translated title (皆无其名) as the same song and prefers the artist named in the title", () => {
    const live = { ...input, title: gbcTitles.BV19T421S7nv, duration: 309, tid: 266 };
    const result = identifyRules(live, [...laroiHits, ...namonakiHits]);
    expect(result).toMatchObject({
      status: "matched",
      song: { name: "名もなき何もかも", artists: ["トゲナシトゲアリ"] },
    });
  });

  it("picks the majority artist among same-title covers when the title names no artist", () => {
    const live = { ...input, title: gbcTitles.BV1dy75z8Enu, duration: 248, tid: 29 };
    const result = identifyRules(live, [...laroiHits, ...hazeteHits]);
    expect(result).toMatchObject({ status: "matched", song: { name: "爆ぜて咲く", artists: ["トゲナシトゲアリ"] } });
    // One original vs one cover stays ambiguous, and so does a tie between two artists.
    expect(identifyRules(live, [hazeteHits[0], hazeteHits[2]]).status).toBe("ambiguous");
    expect(identifyRules(live, [hazeteHits[0], hazeteHits[1], hazeteHits[2], hazeteHits[3]]).status).toBe("ambiguous");
  });

  it("lets a longer matched name cover a song whose name is only its prefix (爆ぜて咲く vs 爆ぜて咲)", () => {
    const live = { ...input, title: "【拉刺】爆ぜて咲く / 爆裂绽放", duration: 253, tid: 29 };
    const prefix = { id: 3442900298, name: "爆ぜて咲", artists: ["Jav @須野高校軽音部"], duration: 213 };
    expect(identifyRules(live, [...hazeteHits, prefix])).toMatchObject({
      status: "matched",
      song: { name: "爆ぜて咲く", artists: ["トゲナシトゲアリ"] },
    });
    // The short name still wins when it is the only song named in the title.
    expect(identifyRules({ ...live, title: "爆ぜて咲 MV" }, [...hazeteHits, prefix])).toMatchObject({
      status: "matched",
      song: { id: prefix.id },
    });
  });

  it("matches an alias without its trailing bracketed annotation (无需既定的未来)", () => {
    const op = {
      ...input,
      title: "【中日歌词/GBC剧场版】命运之花完整版&剧场版OPED【MV】",
      pageTitle: "OP1「无需既定的未来」",
      duration: 94,
      tid: 130,
    };
    const mirai = {
      id: 3320294433,
      name: "もう何もいらない未来",
      artists: ["トゲナシトゲアリ"],
      duration: 219,
      aliases: ["无需既定的未来（少女乐队的呐喊 剧场版『【前篇】 青春狂走曲』主题曲）"],
    };
    const other = { id: 3327845552, name: "もう何もいらない未来", artists: ["トゲナシトゲアリ"], duration: 219 };
    expect(identifyRules(op, [mirai, other])).toMatchObject({ status: "matched", song: { id: mirai.id } });
  });

  // BV1StSoBPEwu (GBC movie OP/ED compilation): NetEase hits recorded on the Mac, 2026-10-29.
  const movie = { ...input, title: "【中日歌词/GBC剧场版】命运之花完整版&剧场版OPED【MV】", tid: 130 };
  const unmeiHits = [
    { id: 2603229013, name: "運命の華", artists: ["トゲナシトゲアリ"], duration: 196, aliases: ["命运之花"] },
    { id: 2621539074, name: "運命の華", artists: ["トゲナシトゲアリ"], duration: 196, aliases: ["命运之花"] },
    { id: 31452205, name: "運命の華", artists: ["结香"], duration: 249, aliases: ["命运之花"] },
    { id: 3371364144, name: "命运之花", artists: ["每天发呆15小时_mini"], duration: 222 },
    { id: 2652039972, name: "運命の華", artists: ["トゲナシトゲアリ"], duration: 196, aliases: ["命运之花"] },
    { id: 1415137515, name: "命运之花", artists: ["YXT"], duration: 211 },
    { id: 3397706804, name: "命运之花", artists: ["康知南"], duration: 142 },
    { id: 2614062525, name: "運命の華", artists: ["水樹愛理"], duration: 282, aliases: ["命运之花"] },
  ];
  const arrowHits = [
    { id: 1821558171, name: "ARROW", artists: ["niki", "Lily"], duration: 233 },
    { id: 2025912142, name: "ARROW (feat. Lily)", artists: ["niki", "Lily"], duration: 232 },
    { id: 3325110244, name: "arrow", artists: ["トゲナシトゲアリ"], duration: 189 },
    { id: 2640733832, name: "Arrow", artists: ["EMBRZ", "bailey"], duration: 245 },
    { id: 2701550323, name: "Arrow", artists: ["stressd"], duration: 124 },
  ];

  it("hands a same-name title to alignment with the most-released artist first", () => {
    const unmei = identifyRules({ ...movie, pageTitle: "命运之花【完整版】", duration: 214 }, unmeiHits);
    expect(unmei.status).toBe("ambiguous");
    if (unmei.status !== "ambiguous") return;
    expect(unmei.contenders.map(c => c.artists[0])).toEqual(["トゲナシトゲアリ", "YXT", "每天发呆15小时_mini"]);
    const arrow = identifyRules({ ...movie, pageTitle: "OP2「arrow」", duration: 90 }, arrowHits);
    expect(arrow.status).toBe("ambiguous");
    if (arrow.status !== "ambiguous") return;
    expect(arrow.contenders.map(c => c.artists[0])).toEqual(["niki", "stressd", "トゲナシトゲアリ"]);
  });

  it("lets a quoted name in a non-music video reach alignment instead of rejecting it", () => {
    const vlog = { ...input, title: "10.2港刺《爆ぜて咲く/爆裂绽放》", duration: 248, tid: 21 };
    expect(identifyRules(vlog, [...laroiHits, ...hazeteHits])).toMatchObject({
      status: "ambiguous",
      contenders: [{ name: "爆ぜて咲く", artists: ["トゲナシトゲアリ"] }],
    });
    expect(identifyQueries(vlog)).toContain("爆ぜて咲く");
    expect(identifyRules({ ...vlog, title: "10.2 港刺 vlog" }, hazeteHits).status).toBe("not-music");
    expect(identifyQueries({ ...vlog, title: "10.2 港刺 vlog" })).toEqual([]);
  });

  it("drops commentary after a full stop from search queries", () => {
    const live = { ...input, title: gbcTitles.BV1dy75z8Enu, duration: 248, tid: 29 };
    const queries = identifyQueries(live);
    expect(queries).toContain("爆ぜて咲く");
    expect(identifyPrimaryQueries(live).filter(q => q.includes("互动拉满"))).toEqual([]);
    expect(identifyPrimaryQueries({ ...input, title: "爆ぜて咲く。" })).toContain("爆ぜて咲く");
  });

  it("keeps series names whole in search queries instead of single English words", () => {
    const sora = buildSearchQueries(gbcTitles.BV1QhieYjEY2);
    expect(sora).toContain("空の箱");
    expect(sora).toContain("トゲナシトゲアリ");
    expect(sora).not.toContain("Girls");
    expect(sora).not.toContain("Band");
    expect(buildSearchQueries(gbcTitles.BV19T421S7nv)).toContain("名もなき何もかも");
    expect(buildSearchQueries(gbcTitles.BV15t421K7Le)).toContain("名もなき何もかも");
    expect(buildSearchQueries(gbcTitles.BV1dy75z8Enu)).toContain("爆ぜて咲く。");
    for (const title of Object.values(gbcTitles)) {
      const queries = [...buildSearchQueries(title), ...identifyQueries({ ...input, title })];
      expect(queries.filter(q => /^(girls|band|cry)$/i.test(q))).toEqual([]);
    }
  });
});
