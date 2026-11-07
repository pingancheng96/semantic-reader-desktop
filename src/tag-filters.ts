export type Tag = {id:string; name:string; aliases:string[]; count:number; scope?:string|null; source_label?:string|null; source_url?:string|null};
export type TagLink = {group_id:string; tag_id:string};

export function facets<T extends {id:string}>(groups:T[], tags:Tag[], links:TagLink[], selected:string[], untagged:boolean) {
  const assigned = new Map<string,Set<string>>();
  for (const link of links) {
    let values = assigned.get(link.group_id);
    if (!values) { values = new Set(); assigned.set(link.group_id,values); }
    values.add(link.tag_id);
  }
  const untaggedCount = groups.filter(group=>!assigned.has(group.id)).length;
  const results = groups.filter(group=>untagged
    ? !assigned.has(group.id)
    : selected.every(tag=>assigned.get(group.id)?.has(tag)));
  const counts = new Map<string,number>();
  for (const group of results) {
    for (const tag of assigned.get(group.id) ?? []) counts.set(tag,(counts.get(tag) ?? 0)+1);
  }
  const options = tags.filter(tag=>counts.has(tag.id) && !selected.includes(tag.id))
    .map(tag=>({...tag,count:counts.get(tag.id)!}));
  return {results,options,untaggedCount};
}
