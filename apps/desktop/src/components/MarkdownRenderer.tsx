import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";

const markdownComponents: Components = {
  a({ node, ...props }) {
    void node;
    return <a {...props} target="_blank" rel="noreferrer" />;
  },
};

/**
 * Heavy parser kept in its own lazy chunk so it does not delay app startup.
 * `components` overrides individual elements (Open loops makes its task
 * items interactive); everything else renders as in notes and summaries.
 */
export default function MarkdownRenderer({
  source,
  components,
}: {
  source: string;
  components?: Components;
}) {
  return (
    <ReactMarkdown
      remarkPlugins={[remarkGfm]}
      components={components ? { ...markdownComponents, ...components } : markdownComponents}
    >
      {source}
    </ReactMarkdown>
  );
}
