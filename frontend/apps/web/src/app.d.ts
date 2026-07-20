export {};

declare global {
  namespace App {
    interface RequestEventLocals {
      cspNonce?: string;
    }
  }
}
