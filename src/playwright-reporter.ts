import { writeFileSync } from 'node:fs'
import type { FullConfig, Reporter, Suite, TestError } from '@playwright/test/reporter'

export interface PlaywrightInventory {
  root: string
  sharedDeclaration?: string
  config?: string
  hooks: string[]
  projects: { name: string; dir: string; dependencies: string[]; teardown?: string; files: string[] }[]
}

// The public reporter config exposes only the first global hook of each kind.
// Read the runner's resolved arrays when hooks are configured. If the runner
// changes this representation, stop discovery before emitting an incomplete list.
function globalHooks(config: FullConfig): string[] {
  if (!config.globalSetup && !config.globalTeardown) return []
  const key = Object.getOwnPropertySymbols(config).find(symbol => symbol.description === 'configInternalSymbol')
  const full = key && Reflect.get(config, key)
  if (!Array.isArray(full?.globalSetups) || !Array.isArray(full.globalTeardowns))
    throw new Error('Cannot enumerate all Playwright global setup and teardown scripts')
  return [...full.globalSetups, ...full.globalTeardowns]
}

export default class InventoryReporter implements Reporter {
  onBegin(config: FullConfig, suite: Suite) {
    const output = process.env.AFFECTED_PLAYWRIGHT_INVENTORY
    if (!output) throw new Error('Missing inventory destination')
    const collected = new Map(suite.suites.map(group => [group.project(), group.suites]))
    let sharedDeclaration: string | undefined
    const projects = config.projects.map(project => {
      const files = (collected.get(project) ?? []).map(file => {
        if (!file.location) throw new Error('Missing Playwright file location')
        const path = file.location.file
        // Playwright's test-list file filter uses the suite file, but its test filter uses the declaration file.
        if (file.allTests().some(test => test.location.file !== path)) sharedDeclaration ??= path
        return path
      })
      return { name: project.name, dir: project.testDir, dependencies: project.dependencies, ...(project.teardown && { teardown: project.teardown }), files }
    })
    const inventory: PlaywrightInventory = { root: config.rootDir, ...(sharedDeclaration && { sharedDeclaration }), ...(config.configFile && { config: config.configFile }), hooks: globalHooks(config), projects }
    writeFileSync(output, JSON.stringify(inventory))
  }

  onError(error: TestError) {
    process.stderr.write((error.message ?? String(error)) + '\n')
  }
}
