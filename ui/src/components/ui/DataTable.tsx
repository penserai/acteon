import { useState } from 'react'
import {
  type ColumnDef,
  type RowData,
  type SortingState,
  useTable,
} from '@tanstack/react-table'
import { ChevronUp, ChevronDown, ChevronsUpDown } from 'lucide-react'
import { cn } from '../../lib/cn'
import { Button } from './Button'
import { EmptyState } from './EmptyState'
import { TableSkeleton } from './Skeleton'
import { dataTableFeatures } from './tableFeatures'
import styles from './DataTable.module.css'

interface DataTableProps<T extends RowData> {
  data: T[]
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  columns: ColumnDef<typeof dataTableFeatures, T, any>[]
  loading?: boolean
  onRowClick?: (row: T) => void
  emptyTitle?: string
  emptyDescription?: string
  pageSize?: number
  serverTotal?: number
  serverOffset?: number
  onPageChange?: (offset: number) => void
}

export function DataTable<T extends RowData>({
  data,
  columns,
  loading,
  onRowClick,
  emptyTitle = 'No data',
  emptyDescription = 'No records found.',
  pageSize = 50,
  serverTotal,
  serverOffset,
  onPageChange,
}: DataTableProps<T>) {
  const [sorting, setSorting] = useState<SortingState>([])
  const isServerPaginated = serverTotal !== undefined

  const table = useTable({
    features: dataTableFeatures,
    data,
    columns,
    state: { sorting },
    onSortingChange: setSorting,
    initialState: { pagination: { pageIndex: 0, pageSize } },
    manualPagination: isServerPaginated,
    rowCount: serverTotal,
  })

  if (loading) return <TableSkeleton rows={5} cols={columns.length} />

  if (data.length === 0) {
    return <EmptyState title={emptyTitle} description={emptyDescription} />
  }

  const rows = table.getRowModel().rows
  const total = isServerPaginated ? serverTotal! : table.getFilteredRowModel().rows.length
  const offset = isServerPaginated ? (serverOffset ?? 0) : table.state.pagination.pageIndex * pageSize
  const showing = Math.min(offset + pageSize, total)

  return (
    <div className={styles.wrapper}>
      <div className={styles.tableContainer}>
        <table className={styles.table}>
          <thead className={styles.thead}>
            {table.getHeaderGroups().map((hg) => (
              <tr key={hg.id} className={styles.headerRow}>
                {hg.headers.map((header) => (
                  <th
                    key={header.id}
                    className={cn(
                      styles.th,
                      header.column.getCanSort() && styles.thSortable,
                    )}
                    onClick={header.column.getToggleSortingHandler()}
                    aria-sort={
                      header.column.getIsSorted() === 'asc' ? 'ascending'
                        : header.column.getIsSorted() === 'desc' ? 'descending'
                        : 'none'
                    }
                  >
                    <div className={styles.headerContent}>
                      <table.FlexRender header={header} />
                      {header.column.getCanSort() && (
                        <span className={styles.sortIcon}>
                          {header.column.getIsSorted() === 'asc' ? <ChevronUp className={styles.chevron} />
                            : header.column.getIsSorted() === 'desc' ? <ChevronDown className={styles.chevron} />
                            : <ChevronsUpDown className={styles.chevron} />}
                        </span>
                      )}
                    </div>
                  </th>
                ))}
              </tr>
            ))}
          </thead>
          <tbody className={styles.tbody}>
            {rows.map((row) => (
              <tr
                key={row.id}
                onClick={() => onRowClick?.(row.original)}
                className={cn(
                  styles.row,
                  onRowClick && styles.rowClickable,
                )}
              >
                {row.getVisibleCells().map((cell) => (
                  <td key={cell.id} className={styles.td}>
                    <table.FlexRender cell={cell} />
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      {total > pageSize && (
        <div className={styles.pagination}>
          <span className={styles.paginationInfo}>Showing {offset + 1}-{showing} of {total.toLocaleString()}</span>
          <div className={styles.paginationButtons}>
            <Button
              variant="secondary"
              size="sm"
              disabled={offset === 0}
              onClick={() => {
                if (isServerPaginated) onPageChange?.(Math.max(0, offset - pageSize))
                else table.previousPage()
              }}
            >
              Previous
            </Button>
            <Button
              variant="secondary"
              size="sm"
              disabled={offset + pageSize >= total}
              onClick={() => {
                if (isServerPaginated) onPageChange?.(offset + pageSize)
                else table.nextPage()
              }}
            >
              Next
            </Button>
          </div>
        </div>
      )}
    </div>
  )
}
