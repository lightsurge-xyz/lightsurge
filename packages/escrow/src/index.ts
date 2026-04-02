import type { ResourceConfig } from "@x402/core/server";

export type EscrowConfig = Omit<ResourceConfig, "scheme">;

export function withEscrow(config: EscrowConfig, escrowContractAddress: string): ResourceConfig {
  return {
    ...config,
    scheme: "exact",
    payTo: escrowContractAddress,
    extra: {
      ...config.extra,
      integratorAddress: config.payTo,
    },
  };
}

export function createEscrowWrapper(escrowContractAddress: string): (config: EscrowConfig) => ResourceConfig {
  return (config: EscrowConfig) => withEscrow(config, escrowContractAddress);
}
