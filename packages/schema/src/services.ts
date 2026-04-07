import { z } from "zod";

export const serviceStatus = z.enum(["active", "unbonding"]);
export type ServiceStatus = z.infer<typeof serviceStatus>;

export const serviceEndpointTransport = z.enum(["https", "mcp"]);
export type ServiceEndpointTransport = z.infer<typeof serviceEndpointTransport>;
export const serviceAcceptedPayment = z.enum(["x402", "mpp"]);
export type ServiceAcceptedPayment = z.infer<typeof serviceAcceptedPayment>;

export const serviceEndpoints = z.object({
  url: z.url(),
  transport: serviceEndpointTransport,
  payment: serviceAcceptedPayment,
});
export type ServiceEndpoints = z.infer<typeof serviceEndpoints>;

export const servicePriceCurrency = z.enum(["USDC"]);
export type ServicePriceCurrency = z.infer<typeof servicePriceCurrency>;
export const servicePricingModel = z.enum(["per-request", "per-token"]);
export type ServicePricingModel = z.infer<typeof servicePricingModel>;

export const servicePricing = z.object({
  currency: servicePriceCurrency,
  amount: z.number(),
  model: servicePricingModel,
});
export type ServicePricing = z.infer<typeof servicePricing>;

export const serviceQuality = z.object({
  rubric: z.string(),
});
export type ServiceQuality = z.infer<typeof serviceQuality>;

export const service = z.object({
  id: z.string(),

  address: z.string(),

  bond: z.bigint(),
  reputation: z.number().min(0).max(1),
  status: serviceStatus.default("active"),

  registeredAt: z.date(),
  lastSlashAt: z.date().nullable(),
  slashCount: z.int().default(0),

  name: z.string(),
  displayName: z.string(),
  description: z.string(),
  icon: z.url().nullable(),

  owner: z.string(),

  endpoints: z.array(serviceEndpoints),
  capabilities: z.array(z.string()),

  pricing: servicePricing,

  quality: serviceQuality,

  createdAt: z.date(),
  updatedAt: z.date().nullable(),
});
export type Service = z.infer<typeof service>;
