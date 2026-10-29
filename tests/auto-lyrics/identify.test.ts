import { describe, expect, it } from "vitest";

import type { IdentifyInput, SongCandidate } from "../../shared/auto-lyrics/identify";

import {
  clean,
  identifyQueries,
  identifyRules,
  nameForms,
  normalize,
  sameName,
  strongMatch,
  titleVariants,
} from "../../shared/auto-lyrics/identify";

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

  it("rejects ambiguous identities, including multiple explicit songs", () => {
    const cover = { ...song, id: 2, artists: ["another artist"] };
    expect(identifyRules({ ...input, title: song.name }, [song, cover]).status).toBe("unidentified");
    expect(identifyRules(input, [song, cover])).toMatchObject({ status: "matched", song });
    const other = { ...song, id: 3, name: "空の箱" };
    expect(identifyRules({ ...input, title: `${song.name} / 空の箱` }, [song, other]).status).toBe("unidentified");
    expect(identifyRules({ ...input, title: song.name, ownerName: cover.artists[0] }, [song, cover]).status).toBe(
      "unidentified",
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
});
