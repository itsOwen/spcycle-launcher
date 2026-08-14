import assert from "node:assert/strict";
import { test } from "node:test";
import {
  addItems,
  editItem,
  MAX_ADD,
  newItem,
  type ItemConfig,
} from "./stash.ts";

const AMMO: ItemConfig = {
  name: "Light Ammo",
  category: "ammo",
  rarity: "common",
  stack: 50,
  durability: -1,
};

const GUN: ItemConfig = {
  name: "Zeus Beam",
  category: "weapons",
  rarity: "legendary",
  stack: 1,
  durability: -1,
};

// a stack of three weapons is a stash the game shows as one gun
test("weapons get a row each rather than a stack", () => {
  const out = addItems([], "WP_G_HVY_Beam_01", GUN, 3);
  assert.equal(out.length, 3);
  assert.deepEqual(
    out.map((i) => i.amount),
    [1, 1, 1],
  );
  assert.equal(new Set(out.map((i) => i.itemId)).size, 3);
});

// topping up first is what keeps a stash from filling with half-empty stacks
test("a partial stack fills before a new one opens", () => {
  const start = [newItem("Ammo_Light", AMMO, 20)];
  const out = addItems(start, "Ammo_Light", AMMO, 120);

  assert.deepEqual(
    out.map((i) => i.amount),
    [50, 50, 40],
  );
  assert.equal(
    out.reduce((n, i) => n + i.amount, 0),
    140,
  );
  // the original array is untouched, so react sees a genuinely new value
  assert.equal(start[0].amount, 20);
});

// perks and mods are the game's to write, and an edit must not quietly drop them
test("an edit preserves everything the game wrote", () => {
  const gun = {
    ...newItem("WP_G_HVY_Beam_01", GUN, 1),
    modData: { m: [{ slot: "optic" }] },
    rolledPerks: ["fast-reload"],
    origin: { t: "raid", p: "bright-sands", g: "g1" },
    insurance: "insured",
  };
  const [out] = editItem([gun], gun.itemId, { amount: 1, durability: 42 });

  assert.equal(out.durability, 42);
  assert.equal(out.itemId, gun.itemId);
  assert.deepEqual(out.modData, { m: [{ slot: "optic" }] });
  assert.deepEqual(out.rolledPerks, ["fast-reload"]);
  assert.deepEqual(out.origin, { t: "raid", p: "bright-sands", g: "g1" });
  assert.equal(out.insurance, "insured");
});

// an unbounded quantity allocates one row per weapon and takes the webview with it
test("an absurd quantity is clamped rather than allocated", () => {
  const out = addItems([], "WP_G_HVY_Beam_01", GUN, 1e8);
  assert.equal(out.length, MAX_ADD);

  const ammo = addItems([], "Ammo_Light", AMMO, 1e8);
  assert.equal(
    ammo.reduce((n, i) => n + i.amount, 0),
    MAX_ADD,
  );
});
