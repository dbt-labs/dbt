/**
 * Downloads data as a CSV file.
 * The CSV includes raw values for proper spreadsheet compatibility.
 * Spreadsheet applications will handle their own formatting.
 *
 * @param headers - Array of column headers
 * @param rows - 2D array of row data (each row is an array of string values)
 * @param filename - Filename (without .csv extension)
 */
export const downloadCsv = (
  headers: string[],
  rows: string[][],
  filename: string,
): void => {
  const csvContent = [headers, ...rows].map((row) => row.join(',')).join('\n');

  const blob = new Blob([csvContent], { type: 'text/csv;charset=utf-8;' });
  const link = document.createElement('a');
  const url = URL.createObjectURL(blob);

  link.setAttribute('href', url);
  link.setAttribute('download', `${filename}.csv`);
  link.style.visibility = 'hidden';

  document.body.appendChild(link);
  link.click();
  document.body.removeChild(link);

  // Clean up the URL object
  URL.revokeObjectURL(url);
};
