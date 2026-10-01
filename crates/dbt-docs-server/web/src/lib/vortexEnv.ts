interface Env {
  PLATFORM: 'nodejs' | 'browser' | 'unknown';
  PRODUCTION: boolean;
  DEV_FILE_MODE: boolean;
  SERVICE?: string;
  VERSION?: string;
}

const browser = (): Env => {
  return {
    PLATFORM: 'browser',
    PRODUCTION: true,
    DEV_FILE_MODE: false,
  };
};

export function getEnv(): Env {
  return browser();
}

export const env = getEnv();
export default env;
