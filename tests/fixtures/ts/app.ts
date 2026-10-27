import { formatPrice } from './price';

export function show(n: number): string {
  return formatPrice(n);
}
