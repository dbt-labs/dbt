export interface Breadcrumb {
  /** Display label for the breadcrumb item. */
  text: string;
  /** Link destination. If omitted, the item renders as a non-interactive span. */
  href?: string;
}
