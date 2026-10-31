import { z } from "zod";

export const alignRequestSchema = z.object({
  studios: z.array(z.instanceof(Float32Array)).min(1).max(3),
  live: z.instanceof(Float32Array),
  lines: z.array(z.object({ idx: z.number().int(), studio_start: z.number().finite(), text: z.string() })),
});

const coarseSchema = z.object({
  offsetSec: z.number().finite(),
  confidence: z.number().finite(),
  bestSim: z.number().optional(),
});
const lineSchema = z.object({
  idx: z.number().int(),
  text: z.string(),
  studioStart: z.number().finite(),
  liveStartSec: z.number().finite(),
  confidence: z.number().finite(),
  uncertain: z.boolean(),
});

const referenceCheckShape = {
  referenceIndex: z.number().int().min(0).max(2),
  referenceConsensus: z.boolean(),
  referenceLags: z.array(
    z.object({
      i: z.number().int(),
      j: z.number().int(),
      lagSec: z.number().finite(),
      spreadSec: z.number().finite(),
      sim: z.number().finite(),
    }),
  ),
};

export const alignResponseSchema = z.discriminatedUnion("kind", [
  z.object({
    kind: z.literal("coarse"),
    coarse: coarseSchema,
    elapsedMs: z.number(),
    memoryPeakKB: z.number(),
    ...referenceCheckShape,
  }),
  z.object({
    kind: z.literal("fine"),
    ...referenceCheckShape,
    coarse: coarseSchema,
    lines: z.array(lineSchema),
    elapsedMs: z.number(),
    memoryPeakKB: z.number(),
  }),
  z.object({ kind: z.literal("error"), error: z.string() }),
]);

export type AlignRequest = z.infer<typeof alignRequestSchema>;
