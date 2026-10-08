export as namespace UmdLib;
export function umdFn(): void {}
declare global {
  interface Window { __umdLib: typeof UmdLib; }
}
