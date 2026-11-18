import { Truthy } from 'lodash';

export function truthy<TType>(value: TType): value is Truthy<TType> {
  return Boolean(value);
}
