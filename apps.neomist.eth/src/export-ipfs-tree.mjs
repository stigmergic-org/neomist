import { mkdir, rename, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { PATHS } from './config.mjs';
import { nodeShard } from './filters.mjs';

const CATEGORY_PAGE_SIZE = 250;
const BEST_RECENT_APPS_CATEGORY_LIMIT = 100;
const BEST_RECENT_APPS_MIN_QUALITY_SCORE = 0.5;
const BEST_RECENT_APPS_CATEGORY = 'Best recent apps';
const BEST_RECENT_APPS_CATEGORY_SLUG = 'best-recent-apps';
const BEST_RECENT_APPS_EXCLUDED_CATEGORIES = new Set(['redirect', 'unknown', 'unavailable']);
const SEARCH_DOC_PAGE_SIZE = 500;
const SEARCH_GRAM_SIZE = 3;

export async function exportIpfsTree(store, outputDir = PATHS.ipfsRootDir) {
  const stagingDir = `${outputDir}.tmp-${process.pid}`;
  await rm(stagingDir, { recursive: true, force: true });
  await mkdir(stagingDir, { recursive: true });

  try {
    const result = await writeExportTree(store, stagingDir);
    await rm(outputDir, { recursive: true, force: true });
    await rename(stagingDir, outputDir);
    return {
      ...result,
      outputDir,
    };
  } catch (error) {
    await rm(stagingDir, { recursive: true, force: true });
    throw error;
  }
}

async function writeExportTree(store, outputDir) {
  const names = store.listExportableNames();
  const versions = store.listContenthashVersionsForNodes(names.map((row) => row.node));
  const versionsByNode = groupVersionsByNode(versions);
  const exportedAnalysisCids = new Set();
  const categoryGroups = new Map();
  const categoryApps = [];
  const searchDocs = [];
  const searchGrams = new Map();
  const generatedAt = new Date().toISOString();

  for (const row of names) {
    const analysis = parseAnalysis(row);
    const [nodeShardA, nodeShardB] = nodeShard(row.node);
    const byNodeRelPath = joinExportPath('names', 'by-node', nodeShardA, nodeShardB, `${row.node}.json`);
    const analysisRelPath = analysis ? analysisPath(analysis.root_cid) : null;
    const analysisSummary = buildAnalysisSummary(analysis, analysisRelPath);
    const record = {
      node: row.node,
      name: row.name,
      parent_name: row.parent_name,
      is_subdomain: Boolean(row.is_subdomain),
      root_cid: row.root_cid,
      title: row.title,
      icon_url: row.icon_url,
      manifest_url: row.manifest_url,
      analysis: analysisSummary,
      contenthashes: versionsByNode.get(row.node) ?? [],
    };

    await writeJsonFile(path.join(outputDir, byNodeRelPath), record);

    const categoryApp = buildCategoryApp(row, analysis, byNodeRelPath, analysisRelPath);
    addCategoryApp(categoryGroups, analysis, categoryApp);
    if (categoryApp && isBestRecentAppCandidate(analysis)) {
      categoryApps.push(categoryApp);
    }
    addSearchDoc(searchDocs, searchGrams, row, analysis);

    if (analysis && !exportedAnalysisCids.has(analysis.root_cid)) {
      await writeJsonFile(path.join(outputDir, analysisRelPath), analysis);
      exportedAnalysisCids.add(analysis.root_cid);
    }
  }

  const specialCategoryGroups = buildSpecialCategoryGroups(categoryApps);
  await writeCategoryGroups(outputDir, categoryGroups, specialCategoryGroups, generatedAt);
  await writeSearchIndex(outputDir, searchDocs, searchGrams, generatedAt);

  const stats = store.getStats();
  await writeJsonFile(path.join(outputDir, 'meta', 'generated.json'), {
    generated_at: generatedAt,
    exported_names: names.length,
    exported_categories: categoryGroups.size + specialCategoryGroups.length,
    exported_search_docs: searchDocs.length,
  });
  await writeJsonFile(path.join(outputDir, 'meta', 'stats.json'), stats);

  return {
    exportedNames: names.length,
    exportedAnalyses: exportedAnalysisCids.size,
    exportedCategories: categoryGroups.size + specialCategoryGroups.length,
    exportedSearchDocs: searchDocs.length,
  };
}

function buildAnalysisSummary(analysis, analysisRelPath) {
  if (!analysis) {
    return null;
  }

  const result = analysis.result;
  return {
    root_cid: analysis.root_cid,
    model: analysis.model,
    analyzed_at: analysis.analyzed_at,
    path: analysisRelPath,
    category: result?.category ?? null,
    category_confidence: result?.category_confidence ?? null,
    summary: result?.summary ?? null,
    quality_tier: result?.quality?.tier ?? null,
    quality_score: result?.quality?.score ?? null,
    security_risk: result?.security?.risk ?? null,
    security_risk_score: result?.security?.risk_score ?? null,
    threat_type: result?.security?.threat_type ?? null,
    safe_to_list: result?.security?.safe_to_list ?? null,
  };
}

function addCategoryApp(categoryGroups, analysis, categoryApp) {
  const result = analysis?.result;
  const category = result?.category;
  if (!category || !categoryApp) {
    return;
  }

  const slug = categorySlug(category);
  const group = categoryGroups.get(category) ?? {
    category,
    slug,
    apps: [],
  };

  group.apps.push(categoryApp);
  categoryGroups.set(category, group);
}

function buildCategoryApp(row, analysis, nameRelPath, analysisRelPath) {
  const result = analysis?.result;
  if (!result) {
    return null;
  }

  return {
    node: row.node,
    name: row.name,
    parent_name: row.parent_name,
    is_subdomain: Boolean(row.is_subdomain),
    title: row.title,
    icon_url: row.icon_url,
    manifest_url: row.manifest_url,
    root_cid: analysis.root_cid,
    name_path: nameRelPath,
    analysis_path: analysisRelPath,
    contenthash_set_block: row.source_block,
    contenthash_set_tx_hash: row.source_tx_hash,
    last_seen_at: row.last_seen_at,
    analyzed_at: analysis.analyzed_at,
    summary: result.summary ?? null,
    category_confidence: result.category_confidence ?? null,
    quality_tier: result.quality?.tier ?? null,
    quality_score: result.quality?.score ?? null,
    security_risk: result.security?.risk ?? null,
    threat_type: result.security?.threat_type ?? null,
    safe_to_list: result.security?.safe_to_list ?? null,
  };
}

function buildSpecialCategoryGroups(categoryApps) {
  if (categoryApps.length === 0) {
    return [];
  }

  return [
    {
      category: BEST_RECENT_APPS_CATEGORY,
      slug: BEST_RECENT_APPS_CATEGORY_SLUG,
      special: true,
      selection: 'most_recent',
      selection_limit: BEST_RECENT_APPS_CATEGORY_LIMIT,
      min_quality_score: BEST_RECENT_APPS_MIN_QUALITY_SCORE,
      sort: 'quality_score_desc',
      apps: [...categoryApps]
        .sort(compareRecentApps)
        .slice(0, BEST_RECENT_APPS_CATEGORY_LIMIT)
        .sort(compareCategoryApps)
    },
  ];
}

function isBestRecentAppCandidate(analysis) {
  const category = analysis?.result?.category;
  const qualityScore = analysis?.result?.quality?.score;
  return category
    && !BEST_RECENT_APPS_EXCLUDED_CATEGORIES.has(String(category).toLowerCase())
    && typeof qualityScore === 'number'
    && qualityScore >= BEST_RECENT_APPS_MIN_QUALITY_SCORE;
}

async function writeCategoryGroups(outputDir, categoryGroups, specialCategoryGroups, generatedAt) {
  const groups = [...categoryGroups.values(), ...specialCategoryGroups]
    .sort((left, right) => left.category.localeCompare(right.category));

  for (const group of groups) {
    group.apps.sort(compareCategoryApps);
    const pages = chunkApps(group.apps, CATEGORY_PAGE_SIZE);
    const pageSummaries = [];

    for (let index = 0; index < pages.length; index += 1) {
      const pageApps = pages[index];
      const fileName = categoryPageFileName(index);
      const rankStart = index * CATEGORY_PAGE_SIZE + 1;
      const rankEnd = rankStart + pageApps.length - 1;
      pageSummaries.push({
        path: fileName,
        count: pageApps.length,
        rank_start: rankStart,
        rank_end: rankEnd,
      });
      await writeJsonFile(path.join(outputDir, 'names', 'by-category', group.slug, fileName), {
        category: group.category,
        slug: group.slug,
        ...categoryGroupMetadata(group),
        generated_at: generatedAt,
        page_size: CATEGORY_PAGE_SIZE,
        page_index: index,
        rank_start: rankStart,
        rank_end: rankEnd,
        count: pageApps.length,
        apps: pageApps,
      });
    }

    await writeJsonFile(path.join(outputDir, 'names', 'by-category', group.slug, 'index.json'), {
      category: group.category,
      slug: group.slug,
      ...categoryGroupMetadata(group),
      generated_at: generatedAt,
      count: group.apps.length,
      page_size: CATEGORY_PAGE_SIZE,
      page_count: pages.length,
      pages: pageSummaries,
    });
  }

  await writeJsonFile(path.join(outputDir, 'names', 'by-category', 'index.json'), {
    generated_at: generatedAt,
    count: groups.length,
    categories: groups.map((group) => ({
      category: group.category,
      slug: group.slug,
      ...categoryGroupMetadata(group),
      count: group.apps.length,
      path: `${group.slug}/index.json`,
      page_size: CATEGORY_PAGE_SIZE,
      page_count: Math.ceil(group.apps.length / CATEGORY_PAGE_SIZE),
    })),
  });
}

function categoryGroupMetadata(group) {
  if (group.special !== true) {
    return {};
  }

  return {
    special: true,
    selection: group.selection,
    selection_limit: group.selection_limit,
    min_quality_score: group.min_quality_score,
    sort: group.sort,
  };
}

function chunkApps(apps, pageSize) {
  const chunks = [];
  for (let index = 0; index < apps.length; index += pageSize) {
    chunks.push(apps.slice(index, index + pageSize));
  }
  return chunks;
}

function categoryPageFileName(index) {
  return `page-${String(index).padStart(4, '0')}.json`;
}

function addSearchDoc(searchDocs, searchGrams, row, analysis) {
  const result = analysis?.result;
  const docId = searchDocs.length;
  const doc = {
    n: row.name,
    c: row.root_cid,
  };
  setIfPresent(doc, 'q', result?.quality?.score);
  setIfPresent(doc, 'k', result?.category);
  setIfPresent(doc, 'r', result?.security?.risk);
  setIfPresent(doc, 't', result?.security?.threat_type);

  searchDocs.push(doc);
  for (const gram of extractSearchGrams([
    row.name,
    row.title,
    result?.category,
    result?.summary,
  ])) {
    const ids = searchGrams.get(gram) ?? [];
    ids.push(docId);
    searchGrams.set(gram, ids);
  }
}

function setIfPresent(target, key, value) {
  if (value != null) {
    target[key] = value;
  }
}

async function writeSearchIndex(outputDir, docs, grams, generatedAt) {
  const docPages = chunkApps(docs, SEARCH_DOC_PAGE_SIZE);
  for (let index = 0; index < docPages.length; index += 1) {
    const pageDocs = docPages[index];
    const idStart = index * SEARCH_DOC_PAGE_SIZE;
    await writeJsonFile(path.join(outputDir, 'search', 'docs', searchPageFileName(index)), {
      schema_version: 1,
      generated_at: generatedAt,
      page_size: SEARCH_DOC_PAGE_SIZE,
      page_index: index,
      id_start: idStart,
      id_end: idStart + pageDocs.length - 1,
      count: pageDocs.length,
      docs: pageDocs,
    });
  }

  const gramEntries = [...grams.entries()].sort(([left], [right]) => left.localeCompare(right));
  const gramCounts = {};
  const gramShards = new Map();
  for (const [gram, ids] of gramEntries) {
    gramCounts[gram] = ids.length;
    const shard = gramShard(gram);
    const shardGrams = gramShards.get(shard) ?? {};
    shardGrams[gram] = ids;
    gramShards.set(shard, shardGrams);
  }

  for (const [shard, shardGrams] of [...gramShards.entries()].sort(([left], [right]) => left.localeCompare(right))) {
    await writeJsonFile(path.join(outputDir, 'search', 'grams', `${shard}.json`), {
      schema_version: 1,
      shard,
      gram_count: Object.keys(shardGrams).length,
      grams: shardGrams,
    });
  }

  await writeJsonFile(path.join(outputDir, 'search', 'index.json'), {
    schema_version: 1,
    generated_at: generatedAt,
    doc_count: docs.length,
    doc_page_size: SEARCH_DOC_PAGE_SIZE,
    doc_page_count: docPages.length,
    doc_path_template: 'docs/page-{page}.json',
    doc_fields: {
      n: 'name',
      c: 'root_cid',
      q: 'quality_score',
      k: 'category',
      r: 'security_risk',
      t: 'threat_type',
    },
    name_path: {
      requires: 'namehash(name)',
      template: 'names/by-node/{node[2:4]}/{node[4:6]}/{node}.json',
    },
    analysis_path: {
      shard: 'cid_tail_4',
      template: 'analysis/by-cid/{cid[-4:-2]}/{cid[-2:]}/{cid}.json',
    },
    gram_size: SEARCH_GRAM_SIZE,
    min_query_length: SEARCH_GRAM_SIZE,
    gram_shard_size: 2,
    gram_path_template: 'grams/{shard}.json',
    searchable_fields: ['name', 'title', 'category', 'summary'],
    gram_count: gramEntries.length,
    grams: gramCounts,
  });
}

function extractSearchGrams(values) {
  const grams = new Set();
  for (const token of searchTokens(values)) {
    for (let index = 0; index <= token.length - SEARCH_GRAM_SIZE; index += 1) {
      grams.add(token.slice(index, index + SEARCH_GRAM_SIZE));
    }
  }
  return grams;
}

function searchTokens(values) {
  return values
    .filter((value) => value != null)
    .flatMap((value) => String(value).toLowerCase().match(/[a-z0-9]+/g) ?? [])
    .filter((token) => token.length >= SEARCH_GRAM_SIZE);
}

function searchPageFileName(index) {
  return `page-${String(index).padStart(4, '0')}.json`;
}

function gramShard(gram) {
  return gram.slice(0, 2).padEnd(2, '_');
}

function compareCategoryApps(left, right) {
  const leftScore = typeof left.quality_score === 'number' ? left.quality_score : -1;
  const rightScore = typeof right.quality_score === 'number' ? right.quality_score : -1;
  if (rightScore !== leftScore) {
    return rightScore - leftScore;
  }
  return left.name.localeCompare(right.name);
}

function compareRecentApps(left, right) {
  const leftBlock = numericValue(left.contenthash_set_block, -1);
  const rightBlock = numericValue(right.contenthash_set_block, -1);
  if (rightBlock !== leftBlock) {
    return rightBlock - leftBlock;
  }

  const leftSeen = Date.parse(left.last_seen_at ?? '') || 0;
  const rightSeen = Date.parse(right.last_seen_at ?? '') || 0;
  if (rightSeen !== leftSeen) {
    return rightSeen - leftSeen;
  }

  return left.name.localeCompare(right.name);
}

function numericValue(value, fallback) {
  if (value == null) {
    return fallback;
  }

  const number = Number(value);
  return Number.isFinite(number) ? number : fallback;
}

function categorySlug(category) {
  return String(category).toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '') || 'unknown';
}

function parseAnalysis(row) {
  if (!row.analysis_json) {
    return null;
  }

  try {
    return {
      root_cid: row.analysis_root_cid,
      model: row.analysis_model,
      analyzed_at: row.analysis_analyzed_at,
      result: JSON.parse(row.analysis_json),
    };
  } catch {
    return null;
  }
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

function cidShard(cid) {
  const key = String(cid).padStart(4, '_');
  return [key.slice(-4, -2), key.slice(-2)];
}

function analysisPath(cid) {
  const [cidShardA, cidShardB] = cidShard(cid);
  return joinExportPath('analysis', 'by-cid', cidShardA, cidShardB, `${cid}.json`);
}

function joinExportPath(...parts) {
  return parts.join('/');
}

async function writeJsonFile(filePath, value) {
  await mkdir(path.dirname(filePath), { recursive: true });
  await writeFile(filePath, `${JSON.stringify(value)}\n`);
}
