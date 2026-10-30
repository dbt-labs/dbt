// Avoid having an export on the first line for istanbul

const capitalizeFirstLetter = (str: string) =>
  str.charAt(0).toUpperCase() + str.slice(1);

export const toTitleCase = (str: string | null | undefined) => {
  if (!str) return '';
  const words = str.split(' ');
  const titleCasedWords = words.map((word) => capitalizeFirstLetter(word));
  return titleCasedWords.join(' ');
};

/** Take in a camel-case string and make title case */
export const camelToTitleCase = (text: string) => {
  const result = text.replace(/([A-Z])/g, ' $1');
  return result.charAt(0).toUpperCase() + result.slice(1);
};

/**
 * Take in a snake_case string and make title case
 */
export const snakeToSentenceCase = (str: string) => {
  return str.charAt(0).toUpperCase() + str.slice(1).toLowerCase().replaceAll('_', ' ');
};
