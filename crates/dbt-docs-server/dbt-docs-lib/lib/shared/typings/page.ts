export type Page<T> = {
  items: T[];
  nextCursor: string | null;
  totalCount: number | null;
};
