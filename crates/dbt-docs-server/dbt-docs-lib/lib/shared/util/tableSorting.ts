type ColumnType = 'number' | 'string' | 'date' | 'enum';

interface EnumOrderMap {
  [key: string]: number;
}

/**
 * Generic table sorting function that can handle various data types and null values
 * @param data Array of objects to sort
 * @param sortKey The key to sort by
 * @param isDescending Whether to sort in descending order
 * @param columnType The type of the column being sorted
 * @param enumOrderMap Optional map for sorting enum values by their order
 * @returns Sorted array
 */
export function sortTableData<T extends Record<string, any>>(
  data: T[],
  sortKey: keyof T,
  isDescending: boolean,
  columnType: ColumnType,
  enumOrderMap?: EnumOrderMap,
): T[] {
  // Add validation for sortKey
  if (data.length > 0 && !(sortKey in data[0])) {
    throw new Error(`Invalid sort key: ${String(sortKey)}`);
  }

  return [...data].sort((a, b) => {
    const aValue = a[sortKey];
    const bValue = b[sortKey];

    // Handle nulls
    if (aValue === null || aValue === undefined) return isDescending ? 1 : -1;
    if (bValue === null || bValue === undefined) return isDescending ? -1 : 1;

    if (columnType === 'number') {
      const aNum = Number(aValue ?? 0);
      const bNum = Number(bValue ?? 0);
      return isDescending ? bNum - aNum : aNum - bNum;
    }

    if (columnType === 'date') {
      return isDescending
        ? bValue.getTime() - aValue.getTime()
        : aValue.getTime() - bValue.getTime();
    }

    if (columnType === 'enum' && enumOrderMap) {
      const aOrder = enumOrderMap[aValue] ?? Number.MAX_SAFE_INTEGER;
      const bOrder = enumOrderMap[bValue] ?? Number.MAX_SAFE_INTEGER;
      return isDescending ? bOrder - aOrder : aOrder - bOrder;
    }

    // Handle strings
    const aStr = String(aValue).toLowerCase();
    const bStr = String(bValue).toLowerCase();
    return isDescending ? bStr.localeCompare(aStr) : aStr.localeCompare(bStr);
  });
}
