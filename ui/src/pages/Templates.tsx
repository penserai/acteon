import { useState } from 'react'
import { PageHeader } from '../components/layout/PageHeader'
import { Tabs } from '../components/ui/Tabs'
import { TemplatesTab } from './templates/TemplatesTab'
import { ProfilesTab } from './templates/ProfilesTab'
import { PlaygroundTab } from './templates/PlaygroundTab'
import styles from './Templates.module.css'

export function Templates() {
  const [activeTab, setActiveTab] = useState('templates')

  return (
    <div>
      <PageHeader
        title="Payload Templates"
        subtitle="Manage reusable payload templates and field profiles"
      />
      <div className={styles.topTabs}>
        <Tabs
          tabs={[
            { id: 'templates', label: 'Templates' },
            { id: 'profiles', label: 'Profiles' },
            { id: 'playground', label: 'Playground' },
          ]}
          active={activeTab}
          onChange={setActiveTab}
        />
      </div>

      {activeTab === 'templates' && <TemplatesTab />}
      {activeTab === 'profiles' && <ProfilesTab />}
      {activeTab === 'playground' && <PlaygroundTab />}
    </div>
  )
}
