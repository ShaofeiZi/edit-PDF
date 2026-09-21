/** Offline desktop build: Scarf analytics is compiled out. */
export function setScarfConfig(
  _scarfEnabled: boolean | null,
  _consentChecker: (service: string, category: string) => boolean,
): void {}

export function firePixel(_pathname: string): void {}

export function resetScarfConfig(): void {}
