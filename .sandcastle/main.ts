import * as sandcastle from "@ai-hero/sandcastle";

const MAX_ITERATIONS = 100;

for (let iteration = 1; iteration <= MAX_ITERATIONS; iteration++) {
  console.log(`\n=== Iteration ${iteration}/${MAX_ITERATIONS} ===\n`);

  // Phase 1: Plan — orchestrator agent analyzes issues and picks parallelizable work
  const plan = await sandcastle.run({
    name: "Planner",
    agent: sandcastle.claudeCode("claude-opus-4-6"),
    promptFile: "./.sandcastle/plan-prompt.md",
    worktree: {
      mode: "none",
    },
  });

  const planMatch = plan.stdout.match(/<plan>([\s\S]*?)<\/plan>/);
  if (!planMatch) {
    throw new Error("Orchestrator did not produce a <plan> tag.\n\n" + plan.stdout);
  }

  const { issues } = JSON.parse(planMatch[1]!) as {
    issues: { number: number; title: string; branch: string }[];
  };

  if (issues.length === 0) {
    console.log("No issues to work on. Exiting.");
    break;
  }

  console.log(`Planning complete. ${issues.length} issue(s) to work in parallel:`);
  for (const issue of issues) {
    console.log(`  #${issue.number}: ${issue.title} → ${issue.branch}`);
  }

  // Phase 2: Execute + Review — implement then review each branch, all in parallel
  const settled = await Promise.allSettled(
    issues.map(async (issue) => {
      await using sandbox = await sandcastle.createSandbox({
        branch: issue.branch,
        copyToSandbox: ["node_modules"],
        hooks: {
          onSandboxReady: [{ command: "npm install" }],
        },
      });

      const result = await sandbox.run({
        name: `Implementer #${issue.number}`,
        agent: sandcastle.claudeCode("claude-sonnet-4-6"),
        promptFile: "./.sandcastle/implement-prompt.md",
        promptArgs: {
          ISSUE_NUMBER: String(issue.number),
          ISSUE_TITLE: issue.title,
          BRANCH: issue.branch,
        },
      });

      if (result.commits.length > 0) {
        await sandbox.run({
          name: `Reviewer #${issue.number}`,
          agent: sandcastle.claudeCode("claude-opus-4-6"),
          promptFile: "./.sandcastle/review-prompt.md",
          promptArgs: {
            ISSUE_NUMBER: String(issue.number),
            ISSUE_TITLE: issue.title,
            BRANCH: issue.branch,
          },
        });
      }

      return result;
    }),
  );

  for (const [i, outcome] of settled.entries()) {
    if (outcome.status === "rejected") {
      console.error(`  ✗ #${issues[i]!.number} (${issues[i]!.branch}) failed: ${outcome.reason}`);
    }
  }

  const completedIssues = settled
    .map((outcome, i) => ({ outcome, issue: issues[i]! }))
    .filter(
      (
        entry,
      ): entry is {
        outcome: PromiseFulfilledResult<Awaited<ReturnType<typeof sandcastle.run>>>;
        issue: (typeof issues)[number];
      } => entry.outcome.status === "fulfilled" && entry.outcome.value.commits.length > 0,
    )
    .map((entry) => entry.issue);

  const completedBranches = completedIssues.map((i) => i.branch);

  console.log(`\nExecution complete. ${completedBranches.length} branch(es) with commits:`);
  for (const branch of completedBranches) {
    console.log(`  ${branch}`);
  }

  if (completedBranches.length === 0) {
    console.log("No commits produced. Nothing to merge.");
    continue;
  }

  // Phase 3: Merge — one agent merges all branches together
  await sandcastle.run({
    name: "Merger",
    maxIterations: 10,
    agent: sandcastle.claudeCode("claude-sonnet-4-6"),
    promptFile: "./.sandcastle/merge-prompt.md",
    promptArgs: {
      BRANCHES: completedBranches.map((b) => `- ${b}`).join("\n"),
      ISSUES: completedIssues.map((i) => `- #${i.number}: ${i.title}`).join("\n"),
    },
    worktree: {
      mode: "none",
    },
  });

  console.log("\nBranches merged.");
}

console.log("\nAll done.");
