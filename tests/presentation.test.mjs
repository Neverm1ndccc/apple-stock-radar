import test from "node:test";
import assert from "node:assert/strict";
import {
  filterTargets,
  selectedVisibleIds,
  selectionProducts,
  percentage,
  errorText,
} from "../src/presentation.ts";

const products = [
  {
    id: "cn-blue",
    locale: "zh_CN",
    model: "iPhone 17",
    capacity: "256GB",
    color: "蓝色",
    sku: "TEST1",
  },
  {
    id: "cn-white",
    locale: "zh_CN",
    model: "iPhone 17",
    capacity: "512GB",
    color: "白色",
    sku: "TEST2",
  },
  {
    id: "hk-blue",
    locale: "zh_HK",
    model: "iPhone 17",
    capacity: "256GB",
    color: "蓝色",
    sku: "TEST3",
  },
];
const targets = products.map((product, i) => ({
  id: String(i),
  product,
  store: { city: "上海", name: "南京东路" },
  enabled: i !== 1,
  state: i === 2 ? "STALE" : "IN_STOCK",
}));
test("region and capacity selection never includes unavailable combinations", () => {
  assert.deepEqual(
    selectionProducts(products, "zh_CN", "iPhone 17", "256GB").map((p) => p.id),
    ["cn-blue"],
  );
  assert.deepEqual(
    selectionProducts(products, "zh_HK", "iPhone 17", "512GB"),
    [],
  );
});
test("hidden or duplicate selections do not silently expand batch creation", () => {
  assert.deepEqual(
    selectedVisibleIds(
      ["cn-blue", "hk-blue", "cn-blue", "missing"],
      ["cn-blue", "cn-white"],
    ),
    ["cn-blue"],
  );
});
test("target search combines terms with region, state, enabled and group filters", () => {
  assert.deepEqual(
    filterTargets(targets, "IPHONE 蓝色 南京", "zh_CN", "IN_STOCK", [
      "0",
      "2",
    ]).map((t) => t.id),
    ["0"],
  );
  assert.deepEqual(
    filterTargets(targets, "", "", "IN_STOCK").map((t) => t.id),
    ["0"],
  );
  assert.deepEqual(
    filterTargets(targets, "", "", "PAUSED").map((t) => t.id),
    ["1"],
  );
  assert.deepEqual(filterTargets(targets, "", "", "", []), []);
});
test("missing samples display no percentage and error messages redact Bark URLs", () => {
  assert.equal(percentage(0, 0), "—");
  assert.equal(percentage(1, 3), "33.3%");
  assert.equal(
    errorText("failed https://api.day.app/TEST_SECRET?x=1"),
    "failed [Bark 地址已隐藏]",
  );
  assert.equal(
    errorText("failed https://open.feishu.cn/open-apis/bot/v2/hook/TEST_SECRET"),
    "failed [飞书 Webhook 已隐藏]",
  );
});
