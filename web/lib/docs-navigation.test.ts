import { describe, expect, it } from "vitest";
import { DOC_TOPICS, docTopicHref, docTopicIsExternal } from "./docs-map";
import { docsTopicIsCurrent } from "./docs-navigation";

function topic(id: string) {
  const value = DOC_TOPICS.find((candidate) => candidate.id === id);
  if (!value) throw new Error(`missing test topic: ${id}`);
  return value;
}

describe("docsTopicIsCurrent", () => {
  it("marks a dedicated docs page current in either website locale", () => {
    expect(docsTopicIsCurrent(topic("modes"), "en", "/en/docs/modes")).toBe(true);
    expect(docsTopicIsCurrent(topic("tools"), "zh", "/zh/docs/tools/")).toBe(true);
  });

  it("does not mark a different page or the docs hub current", () => {
    expect(docsTopicIsCurrent(topic("modes"), "en", "/en/docs/tools")).toBe(false);
    expect(docsTopicIsCurrent(topic("modes"), "en", "/en/docs")).toBe(false);
  });

  it("routes install and providers to their existing first-party pages", () => {
    expect(docTopicHref(topic("install"), "en")).toBe("/en/install");
    expect(docTopicHref(topic("providers"), "zh")).toBe("/zh/models");
    expect(docsTopicIsCurrent(topic("install"), "en", "/en/install/")).toBe(true);
    expect(docsTopicIsCurrent(topic("providers"), "zh", "/zh/models")).toBe(true);
    expect(docTopicIsExternal(topic("install"))).toBe(false);
    expect(docTopicIsExternal(topic("providers"))).toBe(false);
  });

  it("never marks source-document links as local pages", () => {
    expect(docsTopicIsCurrent(topic("guide"), "en", "/en/docs/guide")).toBe(false);
  });

  it("routes former link-out topics to their dedicated docs pages", () => {
    expect(docTopicHref(topic("runtime-api"), "en")).toBe("/en/docs/runtime-api");
    expect(docsTopicIsCurrent(topic("runtime-api"), "en", "/en/docs/runtime-api")).toBe(true);
    expect(docTopicIsExternal(topic("runtime-api"))).toBe(false);
  });
});
