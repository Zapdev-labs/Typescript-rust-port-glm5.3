// @filename: helper.ts
export function helper(): number { return 42; }
// @filename: main.ts
const h = require("./helper");
export const value: number = typeof h === "number" ? h : 0;
