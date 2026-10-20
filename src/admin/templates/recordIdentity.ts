import type { AdminModel } from "./generated/adminSchema";

export type RecordId = number | string;

export function recordId(value: unknown): RecordId | null {
  return typeof value === "string" || typeof value === "number" ? value : null;
}

export function routeIdentity(value: unknown, model: AdminModel | undefined): RecordId | undefined {
  if (typeof value !== "string" || value === "") return undefined;
  const integerKey = model?.fields.find((field) => field.source === "id")?.type === "integer";
  return integerKey ? Number(value) : value;
}

export function selectedIdentity(value: string, options: { id: RecordId }[]): RecordId | null {
  return options.find((option) => String(option.id) === value)?.id ?? null;
}
