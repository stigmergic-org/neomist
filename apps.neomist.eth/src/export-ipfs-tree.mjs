import { mkdir, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { PATHS } from './config.mjs';
import { encodedNameFile, nameShards, nodeShard } from './filters.mjs';

export async function exportIpfsTree(store, outputDir = PATHS.ipfsRootDir) {
  await mkdir(outputDir, { recursive: true });

  const names = store.listExportableNames();
  const versions = store.listContenthashVersionsForNodes(names.map((row) => row.node));
  const versionsByNode = groupVersionsByNode(versions);

  for (const row of names) {
    const record = {
      node: row.node,
      name: row.name,
      parent_name: row.parent_name,
      is_subdomain: Boolean(row.is_subdomain),
      contenthashes: versionsByNode.get(row.node) ?? [],
    };

    const [nodeShardA, nodeShardB] = nodeShard(row.node);
    const [nameShardA, nameShardB] = nameShards(row.name);
    const byNodePath = path.join(outputDir, 'names', 'by-node', nodeShardA, nodeShardB, `${row.node}.json`);
    const byNamePath = path.join(outputDir, 'names', 'by-name', nameShardA, nameShardB, encodedNameFile(row.name));

    await writeJsonFile(byNodePath, record);
    await writeJsonFile(byNamePath, record);
  }

  const stats = store.getStats();
  await writeJsonFile(path.join(outputDir, 'meta', 'generated.json'), {
    generated_at: new Date().toISOString(),
    exported_names: names.length,
  });
  await writeJsonFile(path.join(outputDir, 'meta', 'stats.json'), stats);

  return {
    exportedNames: names.length,
    outputDir,
  };
}

function groupVersionsByNode(rows) {
  const grouped = new Map();
  for (const row of rows) {
    const entries = grouped.get(row.node) ?? [];
    entries.push({
      contenthash_protocol: row.contenthash_protocol,
      root_cid: row.root_cid,
      contenthash_set_block: row.source_block,
      contenthash_set_tx_hash: row.source_tx_hash,
    });
    grouped.set(row.node, entries);
  }
  return grouped;
}

async function writeJsonFile(filePath, value) {
  await mkdir(path.dirname(filePath), { recursive: true });
  await writeFile(filePath, `${JSON.stringify(value, null, 2)}\n`);
}
