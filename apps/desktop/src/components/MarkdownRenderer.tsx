import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";

const markdownComponents: Components = {
  a({ node, ...props }) {
    void node;
    return <a {...props} target="_blank" rel="noreferrer" />;
  },
};

/** Heavy parser kept in its own lazy chunk so it does not delay app startup. */
export default function MarkdownRenderer({ source }: { source: string }) {
  return (
    <ReactMarkdown remarkPlugins={[remarkGfm]} components={markdownComponents}>
      {source}
    </ReactMarkdown>
  );
}
