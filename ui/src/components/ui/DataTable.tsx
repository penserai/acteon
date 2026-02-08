import { useState } from 'react'
import {
  useReactTable,
  getCoreRowModel,
  getSortedRowModel,
  getPaginationRowModel,
  getFilteredRowModel,
  flexRender,
  type ColumnDef,
  type SortingState,
} from '@tanstack/react-table'
import { ChevronUp, ChevronDown, ChevronsUpDown } from 'lucide-react'
import { cn } from '../../lib/cn'
import { Button } from './Button'
import { EmptyState } from './EmptyState'
import { TableSkeleton } from './Skeleton'
import styles from './DataTable.module.css'

interface DataTableProps<T> {
  data: T[]
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  columns: ColumnDef<T, any>[]
  loading?: boolean
  onRowClick?: (row: T) => void
  emptyTitle?: string
  emptyDescription?: string
  pageSize?: number
  serverTotal?: number
  serverOffset?: number
  onPageChange?: (offset: number) => void
}

export function DataTable<T>({
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

  const table = useReactTable({
    data,
    columns,
    state: { sorting },
    onSortingChange: setSorting,
    getCoreRowModel: getCoreRowModel(),
    getSortedRowModel: getSortedRowModel(),
    getPaginationRowModel: isServerPaginated ? undefined : getPaginationRowModel(),
    getFilteredRowModel: getFilteredRowModel(),
    initialState: { pagination: { pageSize } },
  })

  if (loading) return <TableSkeleton rows={5} cols={columns.length} />

  if (data.length === 0) {
    return <EmptyState title={emptyTitle} description={emptyDescription} />
  }

  const rows = table.getRowModel().rows
  const total = isServerPaginated ? serverTotal! : table.getFilteredRowModel().rows.length
  const offset = isServerPaginated ? (serverOffset ?? 0) : table.getState().pagination.pageIndex * pageSize
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
                      {flexRender(header.column.columnDef.header, header.getContext())}
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
                    {flexRender(cell.column.columnDef.cell, cell.getContext())}
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
