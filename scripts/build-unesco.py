"""Project the official UNESCO RDF/XML dump into the bundled English tag index."""
import gzip
import json
import sys
import xml.etree.ElementTree as ET

RDF = "http://www.w3.org/1999/02/22-rdf-syntax-ns#"
SKOS = "http://www.w3.org/2004/02/skos/core#"
LANG = "{http://www.w3.org/XML/1998/namespace}lang"
NS = {"r": RDF, "s": SKOS}


def english(element, name):
    return [" ".join(node.text.split()) for node in element.findall(f"s:{name}", NS)
            if node.get(LANG) == "en" and node.text]


def build(source):
    root = ET.parse(source).getroot()
    concepts = {
        element.get(f"{{{RDF}}}about"): element for element in root
        if any(node.get(f"{{{RDF}}}resource") == SKOS + "Concept"
               for node in element.findall("r:type", NS))
    }
    names = {uri: english(element, "prefLabel")[0] for uri, element in concepts.items()}
    rows = []
    for uri, element in sorted(concepts.items(), key=lambda item: names[item[0]].casefold()):
        label = names[uri]
        parents = [names[key] for node in element.findall("s:broader", NS)
                   if (key := node.get(f"{{{RDF}}}resource")) in names]
        alternatives = english(element, "altLabel")
        notes = english(element, "scopeNote") + english(element, "definition")
        scope = " ".join(notes)
        if parents:
            scope += (" " if scope else "") + "Broader topics: " + ", ".join(parents) + "."
        if alternatives:
            scope += (" " if scope else "") + "Alternative terms: " + ", ".join(alternatives) + "."
        if not scope:
            scope = "UNESCO preferred topic: " + label + "."
        rows.append([label, scope[:500], "https://vocabularies.unesco.org/unesco/en/page/" + uri.rsplit("/", 1)[-1]])
    return rows


if __name__ == "__main__":
    with open(sys.argv[2], "wb") as output:
        output.write(gzip.compress(json.dumps(build(sys.argv[1]), ensure_ascii=False,
                                              separators=(",", ":")).encode(), mtime=0))
