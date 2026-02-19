import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { apiGet, apiPost, apiPut, apiDelete } from '../client'
import type {
  Template,
  TemplateProfile,
  TemplateListResponse,
  TemplateProfileListResponse,
  CreateTemplateRequest,
  UpdateTemplateRequest,
  CreateTemplateResponse,
  CreateProfileRequest,
  UpdateProfileRequest,
  CreateProfileResponse,
  RenderPreviewRequest,
  RenderPreviewResponse,
  TemplateQueryParams,
} from '../../types'

// ---- Templates ----

export function useTemplates(params: TemplateQueryParams = {}) {
  return useQuery({
    queryKey: ['templates', params],
    queryFn: () => apiGet<TemplateListResponse>('/v1/templates', params as Record<string, string | undefined>),
    refetchInterval: 30000,
  })
}

export function useTemplate(id: string | undefined) {
  return useQuery({
    queryKey: ['templates', id],
    queryFn: () => apiGet<Template>(`/v1/templates/${id}`),
    enabled: !!id,
  })
}

export function useCreateTemplate() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (request: CreateTemplateRequest) =>
      apiPost<CreateTemplateResponse>('/v1/templates', request),
    onSuccess: () => void qc.invalidateQueries({ queryKey: ['templates'] }),
  })
}

export function useUpdateTemplate() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ id, body }: { id: string; body: UpdateTemplateRequest }) =>
      apiPut<Template>(`/v1/templates/${id}`, body),
    onSuccess: () => void qc.invalidateQueries({ queryKey: ['templates'] }),
  })
}

export function useDeleteTemplate() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (id: string) => apiDelete(`/v1/templates/${id}`),
    onSuccess: () => void qc.invalidateQueries({ queryKey: ['templates'] }),
  })
}

// ---- Profiles ----

export function useTemplateProfiles(params: TemplateQueryParams = {}) {
  return useQuery({
    queryKey: ['template-profiles', params],
    queryFn: () => apiGet<TemplateProfileListResponse>('/v1/templates/profiles', params as Record<string, string | undefined>),
    refetchInterval: 30000,
  })
}

export function useTemplateProfile(id: string | undefined) {
  return useQuery({
    queryKey: ['template-profiles', id],
    queryFn: () => apiGet<TemplateProfile>(`/v1/templates/profiles/${id}`),
    enabled: !!id,
  })
}

export function useCreateProfile() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (request: CreateProfileRequest) =>
      apiPost<CreateProfileResponse>('/v1/templates/profiles', request),
    onSuccess: () => void qc.invalidateQueries({ queryKey: ['template-profiles'] }),
  })
}

export function useUpdateProfile() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: ({ id, body }: { id: string; body: UpdateProfileRequest }) =>
      apiPut<TemplateProfile>(`/v1/templates/profiles/${id}`, body),
    onSuccess: () => void qc.invalidateQueries({ queryKey: ['template-profiles'] }),
  })
}

export function useDeleteProfile() {
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (id: string) => apiDelete(`/v1/templates/profiles/${id}`),
    onSuccess: () => void qc.invalidateQueries({ queryKey: ['template-profiles'] }),
  })
}

// ---- Render Preview ----

export function useRenderPreview() {
  return useMutation({
    mutationFn: (request: RenderPreviewRequest) =>
      apiPost<RenderPreviewResponse>('/v1/templates/render', request),
  })
}
