import { createContext, type ReactNode, useContext } from 'react';

const LinkPrefixContext = createContext<string | undefined>(undefined);

export const LinkPrefixProvider = ({
  prefix,
  children,
}: {
  prefix: string;
  children: ReactNode;
}) => (
  <LinkPrefixContext.Provider value={prefix}>{children}</LinkPrefixContext.Provider>
);

export const useLinkPrefix = () => useContext(LinkPrefixContext);

export const useLinkPrefixRequired = () => {
  const prefix = useContext(LinkPrefixContext);
  if (prefix === undefined) throw new Error('LinkPrefixProvider not found in tree');
  return prefix;
};
