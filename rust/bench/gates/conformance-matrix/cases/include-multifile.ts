// @filename: firstFile.ts
export function foo(): string { return "a"; }
// @filename: secondFile.ts
import { foo } from "./firstFile";
export const bar: string = foo();
