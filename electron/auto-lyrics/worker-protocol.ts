import { z } from "zod";

export const alignRequestSchema = z.object({
  studio: z.instanceof(Float32Array),
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

export const alignResponseSchema = z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("coarse"), coarse: coarseSchema, elapsedMs: z.number(), memoryPeakKB: z.number() }),
  z.object({
    kind: z.literal("fine"),
    coarse: coarseSchema,
    lines: z.array(lineSchema),
    elapsedMs: z.number(),
    memoryPeakKB: z.number(),
  }),
  z.object({ kind: z.literal("error"), error: z.string() }),
]);

export type AlignRequest = z.infer<typeof alignRequestSchema>;
