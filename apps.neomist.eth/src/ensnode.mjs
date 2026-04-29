const DOMAIN_CHUNK_SIZE = 100;

export async function fetchLatestContenthashBlock({ ensnodeUrl }) {
  const data = await graphqlRequest(
    ensnodeUrl,
    `query LatestContenthashBlock {
      contenthashChangeds(first: 1, orderBy: blockNumber, orderDirection: desc) {
        blockNumber
      }
    }`,
    {},
  );

  const latest = data.contenthashChangeds?.[0]?.blockNumber;
  return latest == null ? null : Number(latest);
}

export async function fetchContenthashEventPage({ ensnodeUrl, cursorBlockExclusive, first }) {
  if (cursorBlockExclusive == null) {
    const data = await graphqlRequest(
      ensnodeUrl,
      `query RecentContenthashes($first: Int!) {
        contenthashChangeds(first: $first, orderBy: blockNumber, orderDirection: desc) {
          id
          blockNumber
          transactionID
          resolver {
            domain {
              id
            }
          }
        }
      }`,
      { first },
    );
    return data.contenthashChangeds ?? [];
  }

  const data = await graphqlRequest(
    ensnodeUrl,
      `query OlderContenthashes($first: Int!, $cursor: Int!) {
        contenthashChangeds(first: $first, orderBy: blockNumber, orderDirection: desc, where: { blockNumber_lt: $cursor }) {
          id
          blockNumber
          transactionID
          resolver {
            domain {
              id
          }
        }
      }
    }`,
    {
      first,
      cursor: cursorBlockExclusive,
    },
  );

  return data.contenthashChangeds ?? [];
}

export async function fetchAllContenthashEventsForBlock({ ensnodeUrl, blockNumber, first }) {
  const all = [];
  let skip = 0;

  while (true) {
    const data = await graphqlRequest(
      ensnodeUrl,
      `query BlockContenthashes($first: Int!, $skip: Int!, $blockNumber: Int!) {
        contenthashChangeds(first: $first, skip: $skip, orderBy: blockNumber, orderDirection: desc, where: { blockNumber: $blockNumber }) {
          id
          blockNumber
          transactionID
          resolver {
            domain {
              id
            }
          }
        }
      }`,
      {
        first,
        skip,
        blockNumber,
      },
    );

    const page = data.contenthashChangeds ?? [];
    if (page.length === 0) {
      break;
    }

    all.push(...page);
    skip += page.length;
  }

  return all;
}

export async function fetchContenthashEventsSince({ ensnodeUrl, fromBlockInclusive, first }) {
  const all = [];
  let skip = 0;

  while (true) {
    const data = await graphqlRequest(
      ensnodeUrl,
      `query RecentContenthashesSince($first: Int!, $skip: Int!, $cursor: Int!) {
        contenthashChangeds(first: $first, skip: $skip, orderBy: blockNumber, orderDirection: desc, where: { blockNumber_gte: $cursor }) {
          id
          blockNumber
          transactionID
          resolver {
            domain {
              id
            }
          }
        }
      }`,
      {
        first,
        skip,
        cursor: fromBlockInclusive,
      },
    );

    const page = data.contenthashChangeds ?? [];
    if (page.length === 0) {
      break;
    }

    all.push(...page);
    skip += page.length;
  }

  return all;
}

export async function fetchDomainsByIds({ ensnodeUrl, ids }) {
  const results = [];
  for (let index = 0; index < ids.length; index += DOMAIN_CHUNK_SIZE) {
    const chunk = ids.slice(index, index + DOMAIN_CHUNK_SIZE);
    const data = await graphqlRequest(
      ensnodeUrl,
      `query DomainsByIds($ids: [String!]) {
        domains(first: 1000, where: { id_in: $ids }) {
          id
          name
          parent {
            name
          }
          resolver {
            address
            contentHash
          }
        }
      }`,
      { ids: chunk },
    );
    results.push(...(data.domains ?? []));
  }
  return results;
}

async function graphqlRequest(endpoint, query, variables) {
  const response = await fetch(endpoint, {
    method: 'POST',
    headers: {
      'content-type': 'application/json',
      'user-agent': 'apps.neomist.eth/1.0',
    },
    body: JSON.stringify({ query, variables }),
  });

  if (!response.ok) {
    throw new Error(`ENSNode request failed with HTTP ${response.status}`);
  }

  const payload = await response.json();
  if (payload.errors?.length) {
    throw new Error(`ENSNode GraphQL error: ${payload.errors[0].message}`);
  }

  return payload.data;
}
