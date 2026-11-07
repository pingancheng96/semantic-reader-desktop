export type WebNode = {type:'text'; start:number; end:number} |
  {type:'element'; tag:string; children:WebNode[]} | {type:'image'; index:number; alt:string};

const captionContainers = new Set(['p','div','span','small','em','i','strong','b']);
const bodyStructure = new Set(['article','section','header','h1','h2','h3','h4','h5','h6','ul','ol','li',
  'table','thead','tbody','tfoot','tr','th','td','blockquote','pre','dl','dt','dd','hr']);

export function captionNodes(nodes: WebNode[], content: string): WeakSet<WebNode> {
  const captions = new WeakSet<WebNode>();

  function scan(siblings: WebNode[], inFigure: boolean): {hasImage:boolean; isStructured:boolean; textLength:number} {
    let afterImage = false;
    let afterBreak = false;
    let hasImage = false;
    let isStructured = false;
    let textLength = 0;

    for (const node of siblings) {
      const tag = node.type === 'element' ? node.tag : '';
      // Some publishers use a dedicated wrapper containing only an image and its caption.
      const imageAndCaption = node.type === 'element' && tag === 'div' &&
        node.children.length === 2 && node.children[0].type === 'image' &&
        node.children[1].type === 'element' && node.children[1].tag === 'p';
      const nested = node.type === 'element'
        ? scan(node.children, inFigure || tag === 'figure' || imageAndCaption)
        : null;
      const image = node.type === 'image' || Boolean(nested?.hasImage);
      const text = node.type === 'text' ? content.slice(node.start,node.end).trim().length : nested?.textLength ?? 0;
      const structured = bodyStructure.has(tag) || Boolean(nested?.isStructured);
      const explicit = tag === 'figcaption';
      const likely = afterImage && !image && !structured && text > 0 &&
        (inFigure || text <= 160) &&
        (captionContainers.has(tag) || (node.type === 'text' && (inFigure || afterBreak)));
      if (explicit || likely) captions.add(node);

      if (image && text === 0) {
        afterImage = true;
        afterBreak = false;
      } else if (afterImage && tag === 'br') {
        afterBreak = true;
      } else if (text || structured || explicit) {
        afterImage = false;
        afterBreak = false;
      }

      hasImage ||= image;
      isStructured ||= structured;
      textLength += text;
    }
    return {hasImage,isStructured,textLength};
  }

  scan(nodes,false);
  return captions;
}
