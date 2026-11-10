export type UsageDataPoint = {
  date: string; // YYYY-MM-DD format
  builds: number;
  reused: number;
  runDuration: number; // in seconds
  creditsUsed: number;
  creditsSaved: number;
  cost: number;
  costSavings: number;
};
